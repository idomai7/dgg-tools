//! `dgg-supervisor`: the supervisor as installed, without a console window
//! (`dgg-logger supervise` is the same, in a console). Everything it has to
//! say goes to the logs.
//!
//! With `--service` it runs as the Windows service the machine install
//! sets up (see `deploy::install::service`), stopping when the service is
//! stopped.

#![windows_subsystem = "windows"]

use {
  ::clap::Parser,
  ::deploy::supervise::{self, Config, Exit, Stop},
  ::std::{
    io, path::PathBuf, process::ExitCode, sync::OnceLock, time::Duration,
  },
};

/// Keep capture and index running against a data directory until stopped
/// or until capture hands off to a newer instance.
#[derive(Parser)]
#[command(name = "dgg-supervisor", version = ::deploy::BUILD)]
struct Args {
  #[arg(long)]
  data: PathBuf,
  #[arg(long)]
  logs: PathBuf,
  /// Where the logger's binaries are (default: beside this program).
  #[arg(long)]
  bin_dir: Option<PathBuf>,
  /// Minutes between compactions.
  #[arg(long, default_value_t = 60)]
  compact_every: u64,
  /// Run as the machine install's Windows service.
  #[arg(long)]
  service: bool,
}

/// The configuration, for the service's entry point, which the system
/// calls without it.
static CONFIG: OnceLock<Config> = OnceLock::new();

fn main() -> ExitCode {
  let args = Args::parse();
  let Some(bin_dir) = args
    .bin_dir
    .or_else(|| Some(::std::env::current_exe().ok()?.parent()?.to_owned()))
  else {
    return ExitCode::FAILURE;
  };
  let mut config = Config::new(&bin_dir, &args.data, &args.logs);
  config.compact_every = Duration::from_secs(args.compact_every * 60);
  if args.service {
    #[cfg(windows)]
    {
      let _ = CONFIG.set(config);
      return match service::start() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
      };
    }
    #[cfg(not(windows))]
    return ExitCode::FAILURE;
  }
  let stop = Stop::new();
  let on_signal = stop.clone();
  let _ = ::ctrlc::set_handler(move || on_signal.request());
  match supervise::run(&config, &stop) {
    Ok(_) => ExitCode::SUCCESS,
    Err(_) => ExitCode::FAILURE,
  }
}

/// Runs `config` until `stop`: what the service does.
fn run(config: &Config, stop: &Stop) -> io::Result<Exit> {
  supervise::run(config, stop)
}

#[cfg(windows)]
mod service {
  use {
    super::{CONFIG, run},
    ::deploy::{install::service::NAME, supervise::Stop},
    ::std::{ffi::OsString, time::Duration},
    ::windows_service::{
      define_windows_service,
      service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState,
        ServiceStatus, ServiceType,
      },
      service_control_handler::{self, ServiceControlHandlerResult},
      service_dispatcher,
    },
  };

  define_windows_service!(entry, service_main);

  /// Hands this thread to the service control manager until the service
  /// stops.
  pub fn start() -> ::windows_service::Result<()> {
    service_dispatcher::start(NAME, entry)
  }

  fn status(state: ServiceState, exit: u32) -> ServiceStatus {
    ServiceStatus {
      service_type: ServiceType::OWN_PROCESS,
      current_state: state,
      controls_accepted: match state {
        ServiceState::Running => {
          ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        }
        _ => ServiceControlAccept::empty(),
      },
      exit_code: ServiceExitCode::Win32(exit),
      checkpoint: 0,
      wait_hint: Duration::from_secs(15),
      process_id: None,
    }
  }

  fn service_main(_arguments: Vec<OsString>) {
    let Some(config) = CONFIG.get() else {
      return;
    };
    let stop = Stop::new();
    let on_control = stop.clone();
    let handler = move |control| match control {
      ServiceControl::Stop | ServiceControl::Shutdown => {
        on_control.request();
        ServiceControlHandlerResult::NoError
      }
      ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
      _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(handle) = service_control_handler::register(NAME, handler) else {
      return;
    };
    let _ = handle.set_service_status(status(ServiceState::Running, 0));
    // A handoff ends the service cleanly too: a newer capture (an
    // upgrade's) has taken over. A failure is reported, so the service's
    // recovery settings restart it.
    let exit = match run(config, &stop) {
      Ok(_) => 0,
      Err(_) => 1,
    };
    let _ = handle.set_service_status(status(ServiceState::Stopped, exit));
  }
}
