//! Capture dgg chat into the journal.

use {
  ::clap::Parser,
  ::dgg_chat::{DEFAULT_URL, Options},
  ::logger::capture::{instance_id, now_ms, run},
  ::std::{path::PathBuf, process::ExitCode},
};

/// Capture dgg chat into the journal.
#[derive(Parser)]
struct Args {
  /// Data directory; the journal goes in `<data>/journal`.
  #[arg(long, default_value = "data")]
  data: PathBuf,
  /// Host name used in the instance id (default: this machine's).
  #[arg(long)]
  host: Option<String>,
  /// Chat socket URL.
  #[arg(long, default_value = DEFAULT_URL)]
  url: String,
  /// Don't send an Origin header.
  #[arg(long)]
  no_origin: bool,
}

#[::tokio::main]
async fn main() -> ExitCode {
  let args = Args::parse();
  let host = args.host.unwrap_or_else(|| {
    ::gethostname::gethostname().to_string_lossy().into_owned()
  });
  let instance = instance_id(&host, now_ms());
  let mut chat = Options {
    url: args.url,
    ..Options::default()
  };
  if args.no_origin {
    chat.origin = None;
  }
  let journal_root = args.data.join("journal");
  eprintln!("capturing as {instance} into {}", journal_root.display());
  match run(chat, journal_root, instance, shutdown()).await {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("journal write failed: {e}");
      ExitCode::FAILURE
    }
  }
}

/// Completes on Ctrl-C, or on SIGTERM on Unix.
async fn shutdown() {
  #[cfg(unix)]
  {
    use ::tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).unwrap();
    ::tokio::select! {
      _ = ::tokio::signal::ctrl_c() => {}
      _ = term.recv() => {}
    }
  }
  #[cfg(not(unix))]
  {
    let _ = ::tokio::signal::ctrl_c().await;
  }
}
