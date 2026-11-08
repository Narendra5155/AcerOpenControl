//! aoc-coolercontrol: CoolerControl device-service plugin for AcerOpenControl.
//!
//! CoolerControl starts this from /var/lib/coolercontrol/plugins/aoc-coolercontrol/
//! and talks gRPC to it over a Unix socket; the plugin forwards to aocd.

mod aocd;
mod service;
#[cfg(test)]
mod tests;

use std::str::FromStr;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::signal::unix::{signal, SignalKind};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

use crate::device_service::v1::device_service_server::DeviceServiceServer;

pub const SERVICE_ID: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_SOCKET: &str = "/tmp/aoc-coolercontrol.sock";
const DEFAULT_AOCD: &str = "/run/acer-open-control/aocd.sock";

pub mod models {
    pub mod v1 {
        tonic::include_proto!("coolercontrol.models.v1");
    }
}
pub mod device_service {
    pub mod v1 {
        tonic::include_proto!("coolercontrol.device_service.v1");
    }
}

struct Args {
    socket: String,
    aocd: String,
    debug: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args { socket: DEFAULT_SOCKET.into(), aocd: DEFAULT_AOCD.into(), debug: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => a.socket = it.next().ok_or_else(|| anyhow::anyhow!("--socket needs a path"))?,
            "--aocd" => a.aocd = it.next().ok_or_else(|| anyhow::anyhow!("--aocd needs a path"))?,
            "-d" | "--debug" => a.debug = true,
            "-V" | "--version" => {
                println!("{SERVICE_ID} {VERSION}");
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!(
                    "{SERVICE_ID} {VERSION}: CoolerControl plugin for AcerOpenControl\n\n\
                     Usage: {SERVICE_ID} [--socket PATH] [--aocd PATH] [--debug]\n\
                     Defaults: --socket {DEFAULT_SOCKET}  --aocd {DEFAULT_AOCD}\n\
                     Normally started by coolercontrold."
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument {other} (try --help)"),
        }
    }
    Ok(a)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    // coolercontrold passes its own log level in CC_LOG.
    let level = if args.debug {
        log::LevelFilter::Debug
    } else {
        std::env::var("CC_LOG").ok().and_then(|l| log::LevelFilter::from_str(&l).ok()).unwrap_or(log::LevelFilter::Info)
    };
    env_logger::Builder::new().filter_level(level).format_timestamp(None).init();
    log::info!("starting {SERVICE_ID} {VERSION} (aocd at {})", args.aocd);

    let aocd = Arc::new(aocd::Aocd::new(&args.aocd));
    let svc = service::AocService::new(aocd.clone(), &args.aocd);

    let _ = tokio::fs::remove_file(&args.socket).await;
    let listener = UnixListener::bind(&args.socket).map_err(|e| anyhow::anyhow!("cannot bind {}: {e}", args.socket))?;
    let shutdown = async {
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
        log::info!("shutting down");
    };
    Server::builder()
        .add_service(DeviceServiceServer::new(svc))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), shutdown)
        .await?;
    // Hand the fans back explicitly; aocd would also do it when our connection closes.
    if let Err(e) = aocd.release().await {
        log::debug!("release on exit: {e}");
    }
    let _ = tokio::fs::remove_file(&args.socket).await;
    Ok(())
}
