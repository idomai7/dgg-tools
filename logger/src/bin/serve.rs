//! Serve this host's journal over HTTP, for other hosts.

use {
  ::clap::Parser,
  ::logger::serve::router,
  ::std::{net::SocketAddr, path::PathBuf, process::ExitCode},
};

/// Serve this host's journal over HTTP, for other hosts. For now, this is
/// how a capture on another host (`capture --peer`) hands off to one here.
#[derive(Parser)]
struct Args {
  /// Data directory; serves `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  /// Address to listen on. Use 0.0.0.0 to accept other hosts, ideally only
  /// over a VPN or tunnel: the token isn't encrypted.
  #[arg(long, default_value = "127.0.0.1:8788")]
  listen: SocketAddr,
  /// Bearer token clients must send.
  #[arg(long, env = "LOGGER_TOKEN", hide_env_values = true)]
  token: String,
}

#[::tokio::main]
async fn main() -> ExitCode {
  let args = Args::parse();
  if args.token.is_empty() {
    eprintln!("serve needs a non-empty --token (or LOGGER_TOKEN)");
    return ExitCode::FAILURE;
  }
  let listener = match ::tokio::net::TcpListener::bind(args.listen).await {
    Ok(listener) => listener,
    Err(e) => {
      eprintln!("can't listen on {}: {e}", args.listen);
      return ExitCode::FAILURE;
    }
  };
  eprintln!("serving {} on {}", args.data.display(), args.listen);
  let app = router(args.data.join("journal"), args.token);
  match ::axum::serve(listener, app).await {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("serve failed: {e}");
      ExitCode::FAILURE
    }
  }
}
