//! Capture dgg chat into the journal.

use {
  ::clap::Parser,
  ::dgg_chat::{DEFAULT_URL, Options},
  ::logger::capture::{Config, Exit, Handoff, instance_id, now_ms, run},
  ::std::{path::PathBuf, process::ExitCode, time::Duration},
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
  /// Seconds a newer instance must be healthy before this one retires.
  #[arg(long, default_value_t = 30)]
  handoff_grace: u64,
  /// Never retire in favour of a newer instance.
  #[arg(long)]
  keep_running: bool,
}

#[::tokio::main]
async fn main() -> ExitCode {
  let args = Args::parse();
  let host = args.host.unwrap_or_else(|| {
    ::gethostname::gethostname().to_string_lossy().into_owned()
  });
  let started = now_ms();
  let instance = instance_id(&host, started);
  let mut chat = Options {
    url: args.url,
    ..Options::default()
  };
  if args.no_origin {
    chat.origin = None;
  }
  let journal_root = args.data.join("journal");
  eprintln!("capturing as {instance} into {}", journal_root.display());
  let config = Config {
    chat,
    journal_root,
    instance,
    started,
    handoff: Handoff {
      grace: Duration::from_secs(args.handoff_grace),
      keep_running: args.keep_running,
      ..Handoff::default()
    },
  };
  match run(config, shutdown()).await {
    Ok(Exit::Shutdown) => ExitCode::SUCCESS,
    Ok(Exit::HandedOff(successor)) => {
      eprintln!("handed off to {successor}");
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("capture failed: {e}");
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
