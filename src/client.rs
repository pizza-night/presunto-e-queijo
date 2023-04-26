use std::{collections::HashMap, future::ready, io, net::SocketAddr, pin::Pin};

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
};

use crate::{
    message::{self, PizzaMessage},
    ui, Str,
};

pub async fn run(
    peers: Vec<(Str, TcpStream)>,
    server: TcpListener,
    external_message: Sender<ui::TuiMessage>,
    internal_message: Receiver<Str>,
) -> io::Result<()> {
    let peers: HashMap<SocketAddr, Peer> = peers
        .into_iter()
        .map(|(name, socket)| {
            let ip = socket.peer_addr().unwrap();
            (ip, Peer::new(name, socket))
        })
        .collect();

    for p in peers.values() {
        let tui_msg = ui::TuiMessage::UserConnected {
            username: p.name.clone(),
        };
        if external_message.send(tui_msg).await.is_err() {
            return Ok(());
        }
    }

    let r = Client {
        peers,
        server,
        external_message,
        internal_message,
    }
    .run()
    .await;

    match r {
        Ok(_) | Err(ClientTermination::UiClosed) => Ok(()),
        Err(ClientTermination::Io(e)) => Err(e),
    }
}

struct Peer {
    name: Str,
    messages: Pin<Box<dyn Stream<Item = PizzaMessage>>>,
    socket: OwnedWriteHalf,
}

impl Peer {
    fn new(name: Str, socket: TcpStream) -> Self {
        let (rx_socket, tx_socket) = socket.into_split();
        Self {
            name,
            messages: Box::pin(receive_messages(rx_socket)),
            socket: tx_socket,
        }
    }
}

type Peers = HashMap<SocketAddr, Peer>;

struct Client {
    peers: Peers,
    server: TcpListener,
    external_message: Sender<ui::TuiMessage>,
    internal_message: Receiver<Str>,
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
                        Ok((peer, message)) => {
                            match message {
                                PizzaMessage::Text { body } => {
                                    let name = &self.peers[&peer].name;
                                    self.notify_new_message(name.clone(), body).await?;
                                },
                            };
                        }
                        Err(addr) => self.handle_disconnect_of(addr).await?,
                    }
                }
                new_message = recv_msg(&mut self.internal_message) => {
                    let disconnected = self.broadcast_message(new_message?).await;
                    for peer in disconnected {
                        self.handle_disconnect_of(peer).await?;
                    }
                }
            }
        }
    }

    async fn notify_new_message(
        &self,
        username: Str,
        message: Str,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::TuiMessage::NewMessage { username, message };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn notify_connected(&self, username: Str) -> Result<(), ClientTermination> {
        let tui_msg = ui::TuiMessage::UserConnected { username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn notify_disconnected(&self, username: Str) -> Result<(), ClientTermination> {
        let tui_msg = ui::TuiMessage::UserDisconnected { username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn broadcast_message(&mut self, message: Str) -> Vec<SocketAddr> {
        let msg = PizzaMessage::Text { body: message };
        stream::iter(
            self.peers
                .iter_mut()
                .map(|(addr, p)| async { (*addr, msg.write(&mut p.socket).await) }),
        )
        .buffer_unordered(16)
        .filter_map(|(addr, result)| ready(result.is_err().then_some(addr)))
        .collect()
        .await
    }

    async fn handle_disconnect_of(&mut self, peer: SocketAddr) -> Result<(), ClientTermination> {
        let peer = self.peers.remove(&peer).unwrap();
        self.notify_disconnected(peer.name).await
    }

    async fn handle_connection_of(
        &mut self,
        socket: TcpStream,
        addr: SocketAddr,
    ) -> Result<(), ClientTermination> {
        let peer = Peer::new(addr.to_string().into(), socket);
        self.notify_connected(peer.name.clone()).await?;
        self.peers.insert(addr, peer);
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

async fn recv_msg(r: &mut Receiver<Str>) -> Result<Str, ClientTermination> {
    r.recv().await.ok_or_else(|| ClientTermination::UiClosed)
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
