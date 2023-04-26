use std::{future::ready, path::PathBuf, process::ExitCode};

use clap::Parser;
use futures::StreamExt;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::spawn_blocking,
};

mod client;
mod message;
mod peer_discovery;
mod ui;

#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value_t = 2504)]
    port: u16,
    #[arg(short, long, default_value = "./config.pizza")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    let Args { port, config } = Args::parse();

    let peers = match peer_discovery::from_config(config).await {
        Ok(peers) => peers,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let (tx_extern_message, rx_extern_message) = mpsc::channel(10);
    let (tx_internal_message, rx_internal_message) = mpsc::channel(10);

    let ui_thread = spawn_blocking(|| ui::ui(rx_extern_message, tx_internal_message));

    let server = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("failed to start the server socket: {e:?}");
            return ExitCode::FAILURE;
        }
    };

    let peers = futures::stream::iter(peers.peers)
        .map(|peer| async move {
            match TcpStream::connect(peer.addr).await {
                Ok(sock) => Some((peer.name, sock)),
                Err(_) => None,
            }
        })
        .buffer_unordered(16)
        .filter_map(ready)
        .collect::<Vec<_>>()
        .await;

    let p2p_client = client::run(peers, server, tx_extern_message, rx_internal_message);

    tokio::select! {
        Err(e) = ui_thread => {
            eprintln!("ui thread panicked: {e:?}");
            ExitCode::FAILURE
        }
        Err(e) = p2p_client => {
            eprintln!("p2p client crashed: {e:?}");
            ExitCode::FAILURE
        }
        else => ExitCode::SUCCESS
    }
}
