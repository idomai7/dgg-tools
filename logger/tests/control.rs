use {
  ::common::temp_dir::TempDir,
  ::logger::control::{self, Older, Status, successor},
  ::logger_journal::Variant,
  ::std::time::Duration,
};

fn status(instance: &str, started: i64, healthy_since: Option<i64>) -> Status {
  Status {
    instance: instance.into(),
    started,
    heartbeat: 1_000,
    healthy_since,
    build: None,
    listening: None,
  }
}

#[test]
fn format() {
  let healthy = status("a", 5, Some(7));
  let text = healthy.encode();
  assert_eq!(
    text,
    "capture-status dgg.0.2\u{1e}\ninstance a\u{1e}\nstarted 5\u{1e}\n\
     heartbeat 1000\u{1e}\nhealthy-since 7\u{1e}\n"
  );
  assert_eq!(Status::decode(&text), Some(healthy));
  let unhealthy = status("a", 5, None);
  assert_eq!(Status::decode(&unhealthy.encode()), Some(unhealthy));
}

#[test]
fn kicks_are_kept_apart() {
  let healthy = status("a", 5, Some(7));
  let text = healthy.encode_as(Variant::Kick);
  assert!(
    text.starts_with("capture-status kick.0.0\u{1e}\ninstance a\u{1e}\n")
  );
  assert_eq!(
    Status::decode_as(&text, Variant::Kick),
    Some(healthy.clone())
  );
  // Each reads only its own.
  assert_eq!(Status::decode(&text), None);
  assert_eq!(Status::decode_as(&healthy.encode(), Variant::Kick), None);

  let dir = TempDir::new("logger-control-kick");
  control::write_as(dir.path(), &healthy, Variant::Kick).unwrap();
  assert_eq!(control::read_all(dir.path()).unwrap(), []);
  let read = control::read_all_as(dir.path(), Variant::Kick).unwrap();
  assert_eq!(read, [healthy]);
}

#[test]
fn decode_tolerance() {
  let header = "capture-status dgg.0.2\u{1e}\n";
  let base = "instance a\u{1e}\nstarted 5\u{1e}\nheartbeat 1000\u{1e}\n";
  let decode = |text: &str| Status::decode(text);
  // Keys it doesn't know are skipped.
  let extra = format!("{header}{base}added x y\u{1e}\n");
  assert_eq!(decode(&extra), Some(status("a", 5, None)));
  for bad in [
    // Within major 0, only the exact version is read.
    format!("capture-status dgg.0.3\u{1e}\n{base}"),
    format!("capture-status dgg.1.0\u{1e}\n{base}"),
    format!("capture-status dgg.0.\u{1e}\n{base}"),
    format!("{header}{base}started 6\u{1e}\n"),
    format!("{header}{base}instance b\u{1e}\n"),
    format!("{header}{base}healthy-since x\u{1e}\n"),
    format!("{header}{base}novalue\u{1e}\n"),
    format!("{header}{base}build \u{1e}\n"),
    format!("{header}{base}build a\u{1e}\nbuild b\u{1e}\n"),
    format!("{header}instance a\u{1e}\nstarted 5\u{1e}\n"),
    format!("{header}instance a/b\u{1e}\nstarted 5\u{1e}\nheartbeat 1\u{1e}\n"),
    format!("{header}{base}").trim_end().into(),
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

#[test]
fn says_what_it_listens_to() {
  let listening = Status {
    listening: Some("wss://chat.example/ws".into()),
    ..status("a", 5, None)
  };
  let text = listening.encode();
  let line = "listening wss://chat.example/ws\u{1e}\n";
  assert!(text.ends_with(line), "{text}");
  assert_eq!(Status::decode(&text), Some(listening));
  // Left out if it isn't one token.
  let spaced = Status {
    listening: Some("two words".into()),
    ..status("a", 5, None)
  };
  assert!(!spaced.encode().contains("listening"));
  // A reader that doesn't know an entry skips it, as an older one skips
  // this.
  let unknown = status("a", 5, None).encode() + "later x\u{1e}\n";
  assert_eq!(Status::decode(&unknown), Some(status("a", 5, None)));
}

#[test]
fn records_end_at_rs_lf() {
  let built = Status {
    build: Some("0.1.0+93b72b343555".into()),
    ..status("a", 5, Some(7))
  };
  let text = built.encode();
  assert_eq!(
    text,
    "capture-status dgg.0.2\u{1e}\ninstance a\u{1e}\nstarted 5\u{1e}\n\
     heartbeat 1000\u{1e}\nhealthy-since 7\u{1e}\n\
     build 0.1.0+93b72b343555\u{1e}\n"
  );
  assert_eq!(Status::decode(&text), Some(built.clone()));
  // As pushed from another host, with a note in its header.
  let noted = text.replacen("dgg.0.2", "dgg.0.2 ; note=x", 1);
  assert_ne!(noted, text);
  assert_eq!(Status::decode(&noted), Some(built));
  let base = "instance a\u{1e}\nstarted 5\u{1e}\nheartbeat 1000\u{1e}\n";
  for bad in [
    // An entry without its end, or another version's form; older versions
    // are no longer read.
    format!("capture-status dgg.0.2\u{1e}\n{base}healthy-since 7\n"),
    "capture-status dgg.0.2\u{1e}\ninstance a\nstarted 5\nheartbeat 1000\n"
      .into(),
    format!("capture-status 0.0\n{base}"),
    format!("capture-status dgg.0.0\n{base}"),
    format!("capture-status dgg.0.2\n{base}"),
  ] {
    assert_eq!(Status::decode(&bad), None, "{bad:?}");
  }
}

#[test]
fn says_which_older_instance_it_took_over_from() {
  let mut old = status("old", 1, Some(2));
  old.build = Some("0.0.1-abc".into());
  let older = status("older", 0, Some(1));
  let mut me = status("me", 5, None);
  let mut seen = Older::default();
  let all = [old.clone(), older.clone(), me.clone()];
  assert_eq!(seen.gone(&me, &all), Vec::<String>::new());
  // Gone while this one isn't healthy: only stopped.
  assert_eq!(
    seen.gone(&me, &[old.clone(), me.clone()]),
    ["older (build not given) stopped"]
  );
  me.healthy_since = Some(6);
  assert_eq!(
    seen.gone(&me, std::slice::from_ref(&me)),
    ["took over from old (build 0.0.1-abc)"]
  );
  // A newer one than this took over, not this.
  let mut seen = Older::default();
  let newer = status("newer", 9, Some(9));
  seen.gone(&me, &[old.clone(), me.clone(), newer.clone()]);
  assert_eq!(
    seen.gone(&me, &[me.clone(), newer]),
    ["old (build 0.0.1-abc) stopped"]
  );
}

#[test]
fn sweeps_away_statuses_long_stale() {
  use ::logger::control::sweepable;

  let (stale, sweep) = (Duration::from_secs(5), Duration::from_secs(600));
  let now = 2_000_000;
  let at = |instance: &str, started, heartbeat, healthy: bool| Status {
    heartbeat,
    ..status(instance, started, healthy.then_some(started))
  };
  let me = at("me", 1_000_000, now, true);
  let dead_older = at("old", 10, now - 600_000, true);
  let just_stale = at("blip", 20, now - 10_000, true);
  let dead_newer = at("bridge", 1_500_000, now - 700_000, false);
  let others = [
    me.clone(),
    dead_older.clone(),
    just_stale.clone(),
    dead_newer.clone(),
  ];
  let swept = |me: &Status, others: &[Status]| {
    let swept = sweepable(me, others, now, stale, sweep);
    swept.iter().map(|s| s.instance.clone()).collect::<Vec<_>>()
  };
  // Those stopped ten minutes or more, older or newer; never itself.
  assert_eq!(swept(&me, &others), ["old", "bridge"]);
  // Not while it isn't healthy itself.
  let connecting = at("me", 1_000_000, now, false);
  assert_eq!(swept(&connecting, &others), [] as [String; 0]);
  // Nor while a newer one is running: that one sweeps.
  let newer = at("new", 1_900_000, now - 1_000, false);
  let with_newer = [others.as_slice(), &[newer]].concat();
  assert_eq!(swept(&me, &with_newer), [] as [String; 0]);
  assert!(
    control::swept_line(&dead_older, now).starts_with("removed old's status")
  );
}
