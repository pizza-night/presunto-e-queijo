#[cfg(feature = "peer-discovery")]
use std::net::IpAddr;
use std::{
    collections::HashMap, future::ready, io, mem::take, net::SocketAddr, pin::pin, time::Duration,
};

use futures::{stream, Stream, StreamExt};
use futures_buffered::FuturesUnorderedBounded;
use thiserror::Error;
use tokio::{
    io::BufReader,
    net::{tcp::OwnedReadHalf, TcpListener, TcpStream},
    sync::mpsc::{Receiver, Sender},
    time::timeout,
};
use tracing::instrument;

use crate::{
    message::{self, PizzaMessage},
    peer_config, ui, Str,
};

#[cfg(feature = "peer-discovery")]
const DEFAULT_PORT: u16 = 2504;

pub struct ClientOpts {
    pub port: u16,
    pub username: Str,
}

pub async fn run(
    initial_peers: peer_config::Peers,
    external_message: Sender<ui::Event>,
    internal_message: Receiver<ui::Request>,
    ClientOpts { port, username }: ClientOpts,
) -> io::Result<()> {
    tracing::debug!(%port, "starting server");
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

    match timeout(
        Duration::from_secs(5),
        client.add_new_peers(initial_peers.iter().map(|(_, addr)| *addr)),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => match e {
            ClientTermination::UiClosed => return Ok(()),
            ClientTermination::Io(e) => return Err(e),
        },
        Err(_) => {}
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

pub use peer::Peer;
mod peer {
    use std::pin::Pin;

    use futures::Stream;
    use tokio::{
        io::BufWriter,
        net::{tcp::OwnedWriteHalf, TcpStream},
    };

    use crate::{message::PizzaMessage, Str};

    use super::{receive_messages, ClientTermination};

    pub struct Peer {
        pub name: Option<Str>,
        pub messages: Pin<Box<dyn Stream<Item = PizzaMessage> + Send + Sync>>,
        socket: BufWriter<OwnedWriteHalf>,
    }

    impl Peer {
        pub fn new<N: Into<Option<Str>>>(name: N, socket: TcpStream) -> Self {
            let (rx_socket, tx_socket) = socket.into_split();
            Self {
                name: name.into(),
                messages: Box::pin(receive_messages(rx_socket)),
                socket: BufWriter::new(tx_socket),
            }
        }

        pub(super) async fn send(&mut self, msg: &PizzaMessage) -> Result<(), ClientTermination> {
            Ok(msg.write(&mut self.socket).await?)
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
    #[tracing::instrument(skip(self))]
    async fn run(mut self) -> Result<(), ClientTermination> {
        loop {
            tokio::select! {
                r = self.server.accept() => {
                    let (socket, addr) = match r {
                        Ok(x) => x,
                        Err(e) => return Err(ClientTermination::Io(e)),
                    };
                    tracing::warn!(%addr, "accepted connection");
                    self.handle_connection_of(socket, addr).await?;
                }
                // None means there are no clients
                Some((addr, msg)) = read_message(&mut self.peers) => {
                    tracing::warn!(%addr, "received message");
                    match msg {
                        Some(message) => self.handle_message(addr, message).await?,
                        None => self.handle_disconnect_of(addr).await?,
                    }
                }
                ui_request = self.ui.recv_ui_request() => {
                    tracing::warn!(?ui_request, "got ui request");
                    self.handle_ui_request(ui_request?).await?;
                }
            }
        }
    }

    #[tracing::instrument(skip(self, message))]
    async fn handle_message(
        &mut self,
        peer: SocketAddr,
        message: PizzaMessage,
    ) -> Result<(), ClientTermination> {
        match message {
            PizzaMessage::Text { body } => {
                self.new_text_message(peer, body).await?;
            }
            #[cfg(feature = "peer-discovery")]
            PizzaMessage::SetName { name } => {
                self.set_name(peer, name).await?;
            }
            #[cfg(feature = "peer-discovery")]
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

    #[instrument(skip(self))]
    async fn handle_disconnect_of(
        &mut self,
        peer_addr: SocketAddr,
    ) -> Result<(), ClientTermination> {
        tracing::debug!("disconnected");
        let peer = self.peers.remove(&peer_addr).unwrap();
        self.ui.user_disconnected(peer_addr, peer.name).await
    }

    #[instrument(skip(self, socket))]
    async fn handle_connection_of(
        &mut self,
        socket: TcpStream,
        from: SocketAddr,
    ) -> Result<(), ClientTermination> {
        tracing::debug!("got a new connection");
        #[cfg_attr(not(feature = "peer-discovery"), allow(unused_mut))]
        let mut peer = Peer::new(None, socket);
        #[cfg(feature = "peer-discovery")]
        {
            use PizzaMessage::{NewPeers, SetName};
            {
                let set_name = SetName {
                    name: take(&mut self.username),
                };
                let r = peer.send(&set_name).await;
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

                if let Err(e) = peer.send(&NewPeers { ipv4, ipv6 }).await {
                    tracing::error!(?e, "sending new peers");
                    return Ok(());
                }
            }
        }
        self.ui.user_connected(from, peer.name.clone()).await?;
        self.peers.insert(from, peer);
        Ok(())
    }

    #[instrument(skip(self))]
    async fn handle_ui_request(
        &mut self,
        ui_request: ui::Request,
    ) -> Result<(), ClientTermination> {
        let disconnected = match ui_request {
            ui::Request::SendMessage { body } => {
                self.broadcast_message(PizzaMessage::Text { body }).await
            }
            ui::Request::ChangeName { name } => {
                #[cfg(feature = "peer-discovery")]
                {
                    self.broadcast_message(PizzaMessage::SetName { name }).await
                }
                #[cfg(not(feature = "peer-discovery"))]
                {
                    let _ = name;
                    vec![]
                }
            }
        };
        for peer in disconnected {
            self.handle_disconnect_of(peer).await?;
        }
        Ok(())
    }
}

impl Client {
    #[tracing::instrument(skip(self))]
    async fn new_text_message(
        &mut self,
        peer: SocketAddr,
        body: Str,
    ) -> Result<(), ClientTermination> {
        tracing::info!("received a new message");
        let name = &self.peers[&peer].name;
        self.ui.new_message(peer, name.clone(), body).await
    }

    #[tracing::instrument(skip(self))]
    async fn set_name(&mut self, peer: SocketAddr, name: Str) -> Result<(), ClientTermination> {
        tracing::info!("setting peer name");
        if let Some(p) = self.peers.get_mut(&peer) {
            if p.name.as_ref() != Some(&name) {
                let new = p.name.insert(name);
                self.ui.change_name(peer, new.clone()).await?;
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

        tracing::info!(count = %ips.len(), "adding new peers");

        let username = take(&mut self.username);
        let Self { peers, ref ui, .. } = self;
        #[cfg(feature = "peer-discovery")]
        let set_name = PizzaMessage::SetName { name: username };
        {
            #[cfg(feature = "peer-discovery")]
            let set_name = &set_name;
            let connected_peers = stream::iter(ips)
            .map(|addr| async move {
                tracing::debug!("connecting to {addr}");
                match timeout(Duration::from_secs(1), TcpStream::connect(addr)).await {
                    Ok(Ok(sock)) => {
                        let addr = sock.peer_addr().unwrap();
                        #[cfg_attr(not(feature = "peer-discovery"), allow(unused_mut))]
                        let mut peer = Peer::new(None, sock);
                        #[cfg(feature = "peer-discovery")]
                        if let Err(e) = peer.send(set_name).await {
                            tracing::error!(to = %addr, ?e, "failed to send set name to new peer");
                        }
                        Some((addr, peer))
                    }
                    _ => None,
                }
            })
            .buffered(16)
            .filter_map(ready)
            .then(|(peer_addr, peer)| async move {
                ui.user_connected(peer_addr, None).await?;

                Ok::<_, ClientTermination>((peer_addr, peer))
            });

            let mut connected_peers = pin!(connected_peers);
            while let Some(connected_peer) = connected_peers.next().await {
                let (addr, peer) = connected_peer?;
                peers.insert(addr, peer);
            }
        }

        #[cfg(feature = "peer-discovery")]
        let PizzaMessage::SetName { name: username } = set_name
        else {
            unreachable!()
        };
        self.username = username;
        Ok(())
    }

    /// Returns the addresses of the peers that failed to receive the message
    #[instrument(skip(self))]
    async fn broadcast_message(&mut self, message: PizzaMessage) -> Vec<SocketAddr> {
        tracing::info!(peers = %self.peers.len(), "broadcasting message");
        stream::iter(self.peers.iter_mut().map(|(addr, p)| {
            let message = &message;
            async move {
                tracing::debug!(?message, to = ?addr, "sending message");
                (*addr, p.send(message).await)
            }
        }))
        .buffer_unordered(16)
        .filter_map(|(addr, result)| {
            tracing::debug!(?message, to = ?addr, "sent message");
            ready(result.is_err().then_some(addr))
        })
        .collect()
        .await
    }
}

async fn read_message(peers: &mut Peers) -> Option<(SocketAddr, Option<PizzaMessage>)> {
    if peers.is_empty() {
        return None;
    }

    FuturesUnorderedBounded::from_iter(peers.iter_mut().map(|(addr, p)| async move {
        match p.messages.next().await {
            Some(msg) => (*addr, Some(msg)),
            None => (*addr, None),
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
                Err(message::ParseError::EvilClient) => {
                    tracing::error!("evil client detected!");
                    // break;
                }
                Err(e) => {
                    tracing::error!(
                        peer = ?buffered.get_ref().peer_addr(),
                        e = ?e,
                        "failed to read message from peer",
                    );
                    continue;
                }
            };
        }
    }
}

struct UiHandle {
    external_message: Sender<ui::Event>,
    internal_message: Receiver<ui::Request>,
}

impl UiHandle {
    async fn new_message(
        &self,
        at: SocketAddr,
        username: Option<Str>,
        message: Str,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::Event::NewMessage {
            at,
            username,
            message,
        };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn user_connected(
        &self,
        at: SocketAddr,
        username: Option<Str>,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::Event::UserConnected { at, username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn user_disconnected(
        &self,
        at: SocketAddr,
        username: Option<Str>,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::Event::UserDisconnected { at, username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn change_name(
        &self,
        at: SocketAddr,
        new_username: Str,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::Event::UpdateUserName { at, new_username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn recv_ui_request(&mut self) -> Result<ui::Request, ClientTermination> {
        self.internal_message
            .recv()
            .await
            .ok_or_else(|| ClientTermination::UiClosed)
    }
}
