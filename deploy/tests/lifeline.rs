//! A program tied to a lifeline dies with it: what keeps a killed
//! supervisor from leaving its programs running.

#[cfg(windows)]
#[test]
fn programs_die_with_their_lifeline() {
  use {
    ::deploy::lifeline::Lifeline,
    ::std::{
      process::{Command, Stdio},
      thread,
      time::{Duration, Instant},
    },
  };

  let lifeline = Lifeline::new().unwrap();
  let mut child = Command::new("ping")
    .args(["-n", "30", "127.0.0.1"])
    .stdout(Stdio::null())
    .spawn()
    .unwrap();
  lifeline.tie(&child).unwrap();
  thread::sleep(Duration::from_millis(300));
  assert!(child.try_wait().unwrap().is_none(), "still running");

  // The supervisor's handle closing, however it ends.
  drop(lifeline);
  let until = Instant::now() + Duration::from_secs(5);
  while child.try_wait().unwrap().is_none() {
    assert!(Instant::now() < until, "outlived its lifeline");
    thread::sleep(Duration::from_millis(50));
  }
}
