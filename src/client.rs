use std::{collections::HashMap, future::ready, io, net::SocketAddr};

use futures::{stream, FutureExt, StreamExt};
use thiserror::Error;
use tokio::{
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
};

use crate::{
    message::{self, PizzaMessage},
    ui, ArcStr,
};

pub async fn run(
    peers: Vec<(ArcStr, TcpStream)>,
    server: TcpListener,
    external_message: Sender<ui::TuiMessage>,
    internal_message: Receiver<ArcStr>,
) -> io::Result<()> {
    let peers: HashMap<SocketAddr, Peer> = peers
        .into_iter()
        .map(|p| {
            let ip = p.1.peer_addr().unwrap();
            (ip, Peer::new(p.0, p.1))
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
    name: ArcStr,
    message_channel: Receiver<PizzaMessage>,
    socket: OwnedWriteHalf,
    _handle: JoinHandle<()>,
}

impl Peer {
    fn new(name: ArcStr, socket: TcpStream) -> Self {
        let (tx, rx) = mpsc::channel(1);
        let (rx_socket, tx_socket) = socket.into_split();
        Self {
            name,
            message_channel: rx,
            socket: tx_socket,
            _handle: tokio::spawn(async move { receive_messages(rx_socket, tx).await }),
        }
    }
}

type Peers = HashMap<SocketAddr, Peer>;

struct Client {
    peers: Peers,
    server: TcpListener,
    external_message: Sender<ui::TuiMessage>,
    internal_message: Receiver<ArcStr>,
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
                    let new_message = new_message?;
                    self.notify_new_message("me".into(), new_message.clone()).await?;
                    let disconnected = self.broadcast_message(new_message).await;
                    for peer in disconnected {
                        self.handle_disconnect_of(peer).await?;
                    }
                }
            }
        }
    }

    async fn notify_new_message(
        &self,
        username: ArcStr,
        message: ArcStr,
    ) -> Result<(), ClientTermination> {
        let tui_msg = ui::TuiMessage::NewMessage { username, message };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn notify_connected(&self, username: ArcStr) -> Result<(), ClientTermination> {
        let tui_msg = ui::TuiMessage::UserConnected { username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn notify_disconnected(&self, username: ArcStr) -> Result<(), ClientTermination> {
        let tui_msg = ui::TuiMessage::UserDisconnected { username };
        self.external_message
            .send(tui_msg)
            .await
            .map_err(|_| ClientTermination::UiClosed)
    }

    async fn broadcast_message(&mut self, message: ArcStr) -> Vec<SocketAddr> {
        stream::iter(self.peers.iter_mut().map(|(addr, p)| async {
            let msg = PizzaMessage::Text {
                body: message.clone(),
            };
            (*addr, msg.write(&mut p.socket).await)
        }))
        .buffer_unordered(16)
        .filter_map(|(addr, result)| ready(result.is_err().then_some(addr)))
        .collect()
        .await
    }

    async fn handle_disconnect_of(&mut self, peer: SocketAddr) -> Result<(), ClientTermination> {
        let peer = self.peers.remove(&peer).unwrap();
        self.notify_disconnected(peer.name.clone()).await
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

    let (message, _, _) = futures::future::select_all(peers.iter_mut().map(|(addr, p)| {
        async {
            match p.message_channel.recv().await {
                Some(msg) => Ok((*addr, msg)),
                None => Err(*addr),
            }
        }
        .boxed()
    }))
    .await;
    Some(message)
}

async fn recv_msg(r: &mut Receiver<ArcStr>) -> Result<ArcStr, ClientTermination> {
    r.recv().await.ok_or_else(|| ClientTermination::UiClosed)
}

async fn receive_messages(mut socket: OwnedReadHalf, message_channel: Sender<PizzaMessage>) {
    loop {
        let msg = match PizzaMessage::read(&mut socket).await {
            Ok(msg) => msg,
            Err(message::ParseError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return;
            }
            Err(e) => {
                eprintln!(
                    "failed to read message from peer {:?}: {e:?}",
                    socket.peer_addr()
                );
                continue;
            }
        };
        if message_channel.send(msg).await.is_err() {
            break;
        };
    }
}
