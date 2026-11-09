use {
  ::logger::control::{Status, successor},
  ::std::time::Duration,
};

fn status(instance: &str, started: i64, healthy_since: Option<i64>) -> Status {
  Status {
    instance: instance.into(),
    started,
    heartbeat: 1_000,
    healthy_since,
    build: None,
  }
}

#[test]
fn format() {
  let healthy = status("a", 5, Some(7));
  let text = healthy.encode();
  assert_eq!(
    text,
    "capture-status 0.0\ninstance a\nstarted 5\nheartbeat 1000\n\
     healthy-since 7\n"
  );
  assert_eq!(Status::decode(&text), Some(healthy));
  let unhealthy = status("a", 5, None);
  assert_eq!(Status::decode(&unhealthy.encode()), Some(unhealthy));
  let built = Status {
    build: Some("0.1.0+93b72b343555".into()),
    ..status("a", 5, Some(7))
  };
  let text = built.encode();
  assert!(text.ends_with("healthy-since 7\nbuild 0.1.0+93b72b343555\n"));
  assert_eq!(Status::decode(&text), Some(built));
}

#[test]
fn decode_tolerance() {
  let base = "instance a\nstarted 5\nheartbeat 1000\n";
  let decode = |text: &str| Status::decode(text);
  // Keys it doesn't know are skipped.
  let extra = format!("capture-status 0.0\n{base}added x y\n");
  assert_eq!(decode(&extra), Some(status("a", 5, None)));
  for bad in [
    // Within major 0, only the exact version is read.
    format!("capture-status 0.1\n{base}"),
    format!("capture-status 1.0\n{base}"),
    format!("capture-status 0.\n{base}"),
    format!("capture-status 0.0\n{base}started 6\n"),
    format!("capture-status 0.0\n{base}instance b\n"),
    format!("capture-status 0.0\n{base}healthy-since x\n"),
    format!("capture-status 0.0\n{base}novalue\n"),
    format!("capture-status 0.0\n{base}build \n"),
    format!("capture-status 0.0\n{base}build a\nbuild b\n"),
    "capture-status 0.0\ninstance a\nstarted 5\n".into(),
    "capture-status 0.0\ninstance a/b\nstarted 5\nheartbeat 1\n".into(),
    format!("capture-status 0.0\n{base}").trim_end().into(),
  ] {
    assert_eq!(decode(&bad), None, "{bad:?}");
  }
}

#[test]
fn successors() {
  let grace = Duration::from_millis(100);
  let stale_after = Duration::from_millis(500);
  let now = 1_000;
  let me = status("m", 10, Some(0));
  let pick = |others: &[Status]| {
    successor(&me, others, now, grace, stale_after)
      .map(|status| status.instance.clone())
  };
  // Older, unhealthy, not healthy for long enough: no handoff.
  assert_eq!(pick(&[status("old", 9, Some(0))]), None);
  assert_eq!(pick(&[status("new", 11, None)]), None);
  assert_eq!(pick(&[status("new", 11, Some(901))]), None);
  // Healthy for the grace period: hand off.
  assert_eq!(pick(&[status("new", 11, Some(900))]), Some("new".into()));
  // A stale heartbeat means it probably crashed.
  let mut stale = status("new", 11, Some(0));
  stale.heartbeat = 499;
  assert_eq!(pick(&[stale]), None);
  // Same start time: the id breaks the tie.
  assert_eq!(pick(&[status("l", 10, Some(0))]), None);
  assert_eq!(pick(&[status("n", 10, Some(0))]), Some("n".into()));
  // Several candidates: the latest-started wins.
  let candidates = [status("x", 12, Some(0)), status("y", 15, Some(0))];
  assert_eq!(pick(&candidates), Some("y".into()));
  // Its own status is never its successor.
  assert_eq!(pick(::std::slice::from_ref(&me)), None);
}

#[cfg(windows)]
#[test]
fn remove_waits_out_a_reader_that_blocks_deletion() {
  use {
    ::logger::control,
    ::std::{fs, os::windows::fs::OpenOptionsExt, thread},
  };

  let dir = ::std::env::temp_dir()
    .join(format!("logger-control-remove-{}", ::std::process::id()));
  let _ = fs::remove_dir_all(&dir);
  control::write(&dir, &status("a", 5, None)).unwrap();
  let path = control::path(&dir, "a");
  // Open with no sharing, as a virus scanner might: deleting fails until
  // it's closed.
  let held = fs::OpenOptions::new()
    .read(true)
    .share_mode(0)
    .open(&path)
    .unwrap();
  assert!(fs::remove_file(&path).is_err());
  let reader = thread::spawn(move || {
    thread::sleep(Duration::from_millis(300));
    drop(held);
  });
  control::remove(&dir, "a").unwrap();
  reader.join().unwrap();
  assert!(!path.exists());
  // Already gone is fine.
  control::remove(&dir, "a").unwrap();
  fs::remove_dir_all(&dir).unwrap();
}
