use std::{fs::File, net::SocketAddr, path::PathBuf, process::ExitCode, sync::Arc};

use clap::Parser;
use futures::future::OptionFuture;
use presunto_e_queijo::{
    client::{self, ClientOpts},
    peer_config, ui,
};
use tokio::{sync::mpsc, task::spawn_blocking};
use tracing::Level;

#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value_t = 2504)]
    port: u16,
    #[arg(short, long)]
    config: Option<PathBuf>,
    #[arg(short, long)]
    initial_peer: Option<SocketAddr>,
    #[arg(short, long)]
    username: Option<String>,
    #[arg(long)]
    debug_logs: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Args {
        port,
        config,
        initial_peer,
        username,
        debug_logs,
    } = Args::parse();

    if let Some(debug_logs) = debug_logs {
        enable_logging(debug_logs);
    }

    let username = username.unwrap_or_else(whoami::username);

    let mut peers = match OptionFuture::from(config.map(peer_config::load))
        .await
        .unwrap_or_else(|| Ok(Default::default()))
    {
        Ok(peers) => peers,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(initial_peer) = initial_peer {
        peers.push((None, initial_peer));
    }

    let (tx_extern_message, rx_extern_message) = mpsc::channel(10);
    let (tx_internal_message, rx_internal_message) = mpsc::channel(10);

    let ui_thread = spawn_blocking(|| ui::ui(rx_extern_message, tx_internal_message));

    let p2p_client = client::run(
        peers,
        tx_extern_message,
        rx_internal_message,
        ClientOpts {
            port,
            username: username.into(),
        },
    );

    tokio::select! {
        r = ui_thread => {
            if let Err(e) = r {
                match e.try_into_panic() {
                    Ok(panic) => {
                        if let Some(s) = panic.downcast_ref::<String>() {
                            eprintln!("ui thread panicked: {s:?}");
                        } else {
                            eprintln!("ui thread panicked: {panic:?}");
                        }
                    }
                    Err(e) => eprintln!("ui thread aborted: {e:?}"),
                }
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) = p2p_client => {
            eprintln!("p2p client crashed: {e:?}");
            ExitCode::FAILURE
        }
        else => ExitCode::SUCCESS
    }
}

fn enable_logging(path: PathBuf) {
    use tracing_subscriber::{
        filter::EnvFilter, layer::SubscriberExt, util::SubscriberInitExt, Layer,
    };

    let file = File::create(path).expect("failed to open log file");
    let layer = tracing_subscriber::fmt::layer()
        .pretty()
        .with_writer(Arc::new(file))
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(Level::DEBUG.into())
                .from_env_lossy(),
        )
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.module_path()
                .filter(|p| p.starts_with("presunto_e_queijo"))
                .is_some()
        }));

    tracing_subscriber::registry().with(layer).init();
}
