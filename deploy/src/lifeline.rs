//! Ties the programs a supervisor starts to its own life, so even a
//! supervisor killed outright (Task Manager, a hard task stop) leaves none
//! of them running unsupervised.
//!
//! On Windows it's a job object set to kill everything in it when its last
//! handle closes, and the supervisor holds the only handle; the system
//! closes it whatever way the supervisor ends. Elsewhere it does nothing
//! for now.

use ::std::{io, process::Child};

pub struct Lifeline {
  #[cfg(windows)]
  job: ::win32job::Job,
}

impl Lifeline {
  pub fn new() -> io::Result<Self> {
    #[cfg(windows)]
    {
      let mut limits = ::win32job::ExtendedLimitInfo::new();
      limits.limit_kill_on_job_close();
      let job = ::win32job::Job::create_with_limit_info(&limits)
        .map_err(|e| io::Error::other(format!("job object: {e}")))?;
      Ok(Self { job })
    }
    #[cfg(not(windows))]
    Ok(Self {})
  }

  /// Ties `child` to this lifeline.
  pub fn tie(&self, child: &Child) -> io::Result<()> {
    #[cfg(windows)]
    {
      use ::std::os::windows::io::AsRawHandle;
      self
        .job
        .assign_process(child.as_raw_handle() as isize)
        .map_err(|e| io::Error::other(format!("job object: {e}")))
    }
    #[cfg(not(windows))]
    {
      let _ = child;
      Ok(())
    }
  }
}
