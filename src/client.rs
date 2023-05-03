use std::{
    collections::HashMap,
    future::ready,
    io,
    mem::take,
    net::{IpAddr, SocketAddr},
    pin::{pin, Pin},
    time::Duration,
};

use futures::{stream, Stream, StreamExt};
use futures_buffered::FuturesUnorderedBounded;
use thiserror::Error;
use tokio::{
    io::BufReader,
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    sync::mpsc::{Receiver, Sender},
    time::timeout,
};
use tracing::instrument;

use crate::{
    message::{self, PizzaMessage},
    peer_config, ui, Str,
};

const DEFAULT_PORT: u16 = 2504;

pub struct ClientOpts {
    pub port: u16,
    pub username: Str,
}

pub async fn run(
    initial_peers: peer_config::Peers,
    external_message: Sender<ui::UiEvent>,
    internal_message: Receiver<Str>,
    ClientOpts { port, username }: ClientOpts,
) -> io::Result<()> {
    tracing::debug!("starting server");
    let server = TcpListener::bind(("0.0.0.0", port)).await?;

    let mut client = Client {
        username,
        peers: HashMap::with_capacity(initial_peers.len()),
        server,
        ui: UiHandle {
            external_message,
            internal_message,
        },
    };

    if let Err(e) = client
        .add_new_peers(initial_peers.iter().map(|(_, addr)| *addr))
        .await
    {
        match e {
            ClientTermination::UiClosed => return Ok(()),
            ClientTermination::Io(e) => return Err(e),
        }
    }

    for (name, addr) in initial_peers
        .into_iter()
        .filter_map(|(n, a)| n.map(|n| (n, a)))
    {
        if let Err(e) = client.set_name(addr, name).await {
            match e {
                ClientTermination::UiClosed => return Ok(()),
                ClientTermination::Io(e) => return Err(e),
            }
        };
    }

    match client.run().await {
        Ok(_) | Err(ClientTermination::UiClosed) => Ok(()),
        Err(ClientTermination::Io(e)) => Err(e),
    }
}

struct Peer {
    name: Option<Str>,
    messages: Pin<Box<dyn Stream<Item = PizzaMessage>>>,
    socket: OwnedWriteHalf,
}

impl Peer {
    fn new<N: Into<Option<Str>>>(name: N, socket: TcpStream) -> Self {
        let (rx_socket, tx_socket) = socket.into_split();
        Self {
            name: name.into(),
            messages: Box::pin(receive_messages(rx_socket)),
            socket: tx_socket,
        }
    }
}

type Peers = HashMap<SocketAddr, Peer>;

struct Client {
    username: Str,
    peers: Peers,
    server: TcpListener,
    ui: UiHandle,
}

#[derive(Error, Debug)]
enum ClientTermination {
    #[error("ui closed")]
    UiClosed,
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl Client {
    async fn run(mut self) -> Result<(), ClientTermination> {
        loop {
            tokio::select! {
                r = self.server.accept() => {
                    let (socket, addr) = match r {
                        Ok(x) => x,
                        Err(e) => return Err(ClientTermination::Io(e)),
                    };
                    self.handle_connection_of(socket, addr).await?;
                }
                // None means there are no clients
                Some(recv_msg) = read_message(&mut self.peers) => {
                    match recv_msg {
                        Ok((peer, message)) => self.handle_message(peer, message).await?,
                        Err(addr) => self.handle_disconnect_of(addr).await?,
                    }
                }
                new_message = self.ui.recv_msg() => {
                    let disconnected = self.broadcast_message(new_message?).await;
                    for peer in disconnected {
                        self.handle_disconnect_of(peer).await?;
                    }
                }
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn handle_message(
        &mut self,
        peer: SocketAddr,
        message: PizzaMessage,
    ) -> Result<(), ClientTermination> {
        match message {
            PizzaMessage::Text { body } => {
                self.new_text_message(peer, body).await?;
            }
            PizzaMessage::SetName { name } => {
                self.set_name(peer, name).await?;
            }
            PizzaMessage::NewPeers { ipv4, ipv6 } => {
                self.add_new_peers(
                    ipv4.into_iter()
                        .map(IpAddr::from)
                        .chain(ipv6.into_iter().map(IpAddr::from))
                        .map(|ip| SocketAddr::new(ip, DEFAULT_PORT)),
                )
                .await?;
            }
        };
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn new_text_message(
        &mut self,
        peer: SocketAddr,
        body: Str,
    ) -> Result<(), ClientTermination> {
        let name = &self.peers[&peer].name;
        self.ui.notify_new_message(peer, name.clone(), body).await
    }

    #[tracing::instrument(skip(self))]
    async fn set_name(&mut self, peer: SocketAddr, name: Str) -> Result<(), ClientTermination> {
        if let Some(p) = self.peers.get_mut(&peer) {
            if p.name.as_ref() != Some(&name) {
                let new = p.name.insert(name);
                self.ui.notify_change_name(peer, new.clone()).await?;
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self, ips))]
    async fn add_new_peers(
        &mut self,
        ips: impl Iterator<Item = SocketAddr>,
    ) -> Result<(), ClientTermination> {
        let ips = ips
            .filter(|ip| !self.peers.contains_key(ip))
            .collect::<Vec<_>>();

        let username = take(&mut self.username);
        let Self { peers, ref ui, .. } = self;
        let set_name = PizzaMessage::SetName { name: username };
        {
            let set_name = &set_name;
            let connected_peers = stream::iter(ips)
            .map(|addr| async move {
                match timeout(Duration::from_secs(5), TcpStream::connect(addr)).await {
                    Ok(Ok(mut sock)) => {
                        if let Err(e) = set_name.write(&mut sock).await {
                            tracing::error!(to = %addr, ?e, "failed to send set name to new peer");
                        }
                        Some(sock)
                    }
                    _ => None,
                }
            })
            .buffered(16)
            .filter_map(ready)
            .then(|sock| async move {
                let peer_addr = sock.peer_addr().unwrap();
                let peer = Peer::new(None, sock);

                ui.notify_connected(peer_addr, None).await?;

                Ok::<_, ClientTermination>((peer_addr, peer))
            });

            let mut connected_peers = pin!(connected_peers);
            while let Some(connected_peer) = connected_peers.next().await {
                let (addr, peer) = connected_peer?;
                peers.insert(addr, peer);
            }
        }

        let PizzaMessage::SetName { name } = set_name else {
            unreachable!()
        };
        self.username = name;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn broadcast_message(&mut self, message: Str) -> Vec<SocketAddr> {
        let msg = PizzaMessage::Text { body: message };
        stream::iter(self.peers.iter_mut().map(|(addr, p)| {
            tracing::debug!(?msg, to = ?addr, "sending message");
            async { (*addr, msg.write(&mut p.socket).await) }
        }))
        .buffer_unordered(16)
        .filter_map(|(addr, result)| ready(result.is_err().then_some(addr)))
        .collect()
        .await
    }

    #[instrument(skip(self))]
    async fn handle_disconnect_of(&mut self, peer: SocketAddr) -> Result<(), ClientTermination> {
        self.peers.remove(&peer).unwrap();
        tracing::debug!("disconnected");
        self.ui.notify_disconnected(peer).await
    }

    #[instrument(skip(self, socket))]
    async fn handle_connection_of(
        &mut self,
        socket: TcpStream,
        from: SocketAddr,
    ) -> Result<(), ClientTermination> {
        tracing::debug!("got a new connection");
        let mut peer = Peer::new(None, socket);
        use PizzaMessage::{NewPeers, SetName};
        {
            let set_name = SetName {
                name: take(&mut self.username),
            };
            let r = set_name.write(&mut peer.socket).await;
            let SetName { name } = set_name else {
                unreachable!()
            };
            self.username = name;

            if let Err(e) = r {
                tracing::error!(?e, "sending set name");
                return Ok(());
            }
        }
        {
            let (ipv4, ipv6) = self.peers.keys().fold(
                (Vec::default(), Vec::default()),
                |(mut v4, mut v6), peer| {
                    match peer.ip() {
                        IpAddr::V4(ip) => v4.push(ip),
                        IpAddr::V6(ip) => v6.push(ip),
                    }
                    (v4, v6)
                },
            );

            if let Err(e) = PizzaMessage::write(&NewPeers { ipv4, ipv6 }, &mut peer.socket).await {
                tracing::error!(?e, "sending new peers");
                return Ok(());
            }
        }
        self.ui.notify_connected(from, peer.name.clone()).await?;
        self.peers.insert(from, peer);
        Ok(())
    }
}

async fn read_message(peers: &mut Peers) -> Option<Result<(SocketAddr, PizzaMessage), SocketAddr>> {
    if peers.is_empty() {
        return None;
    }

    FuturesUnorderedBounded::from_iter(peers.iter_mut().map(|(addr, p)| async {
        match p.messages.next().await {
            Some(msg) => Ok((*addr, msg)),
            None => Err(*addr),
        }
    }))
    .next()
    .await
}

fn receive_messages(socket: OwnedReadHalf) -> impl Stream<Item = PizzaMessage> {
    async_stream::stream! {
        let mut buffered = BufReader::new(socket);
        loop {
            match PizzaMessage::read(&mut buffered).await {
                Ok(msg) => yield msg,
                Err(message::ParseError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => {
                    eprintln!(
                        "failed to read message from peer {:?}: {e:?}",
                        buffered.get_ref().peer_addr()
                    );
                    continue;
                }
            };
        }
    }
}

struct UiHandle {
    external_message: Sender<ui::UiEvent>,
    internal_message: Receiver<Str>,
}

impl UiHandle {
    async fn notify_new_message(
        &self,
        at: SocketAddr,
        username: Option<Str>,
        message: Str,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::UiEvent::NewMessage {
            at,
            username,
            message,
        };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn notify_connected(
        &self,
        at: SocketAddr,
        username: Option<Str>,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::UiEvent::UserConnected { at, username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn notify_disconnected(&self, at: SocketAddr) -> Result<(), ClientTermination> {
        let tui_msg = ui::UiEvent::UserDisconnected { at };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn notify_change_name(
        &self,
        at: SocketAddr,
        new_username: Str,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::UiEvent::UpdateUserName { at, new_username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn recv_msg(&mut self) -> Result<Str, ClientTermination> {
        self.internal_message
            .recv()
            .await
            .ok_or_else(|| ClientTermination::UiClosed)
    }
}
