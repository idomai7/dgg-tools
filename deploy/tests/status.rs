//! The status report, against a made-up data directory.

use {
  ::common::temp_dir::TempDir,
  ::common::time::{now_ms, utc_day},
  ::deploy::{
    paths::Paths,
    status::{
      Health, Watcher, all_logs, capture_tail, gaps, journal_lines, tail, text,
    },
  },
};

fn status(
  instance: &str,
  started: i64,
  heartbeat: i64,
  healthy: Option<i64>,
) -> String {
  let status = ::logger::control::Status {
    instance: instance.into(),
    started,
    heartbeat,
    healthy_since: healthy,
    build: None,
  };
  status.encode()
}

#[test]
fn reports_captures_and_the_journal() {
  let dir = TempDir::new("deploy-status-report");
  let now = now_ms();
  let today = utc_day(now);
  let control = "data/journal/control";
  dir.write(
    format!("{control}/a-1.status"),
    status("a-1", now - 3_600_000, now - 500, Some(now - 3_500_000)),
  );
  dir.write(
    format!("{control}/a-2.status"),
    status("a-2", now - 120_000, now - 60_000, None),
  );
  dir.write(
    format!("{control}/a-3.status"),
    status("a-3", now - 2000, now - 100, None),
  );
  // Two frames, a connection opening, and a frame still being written.
  dir.write(
    format!("data/journal/live/{today}/a-3.journal"),
    "journal dgg.0.3\u{1e}\n1 a-3 1 open wss://x\u{1e}\n2 a-3 1 frame MSG {}\u{1e}\n\
     3 a-3 1 frame NAMES {}\u{1e}\n4 a-3 1 frame JO",
  );
  dir.write(
    format!("data/journal/live/{today}/a-1.journal"),
    "0123456789",
  );
  dir.write("data/journal/live/2026-01-02/a-1.journal", "01234");
  dir.write("data/journal/backfill/live/2026-01-01/b.journal", "012");
  dir.write(
    "data/journal/archive/2026/01/manifest",
    format!(
      "journal-archive dgg.0.2\u{1e}\n2026-01-01 100 1000 10 {0}\u{1e}\n\
       2026-01-03 50 500 5 {0}\u{1e}\n",
      "0".repeat(64)
    ),
  );
  dir.write("data/index.sqlite", "x");

  let paths = Paths::under(dir.to_path_buf());
  let mut watcher = Watcher::new().unwrap();
  let report = watcher.gather(&paths).unwrap();

  let health: Vec<_> = report
    .captures
    .iter()
    .map(|capture| (capture.instance.as_str(), capture.health))
    .collect();
  assert_eq!(
    health,
    [
      ("a-1", Health::Healthy(now - 3_500_000)),
      ("a-2", Health::Stale(now - 60_000)),
      ("a-3", Health::Connecting),
    ]
  );
  let frames: Vec<_> = report.captures.iter().map(|c| c.frames).collect();
  assert_eq!(frames, [0, 0, 2]);
  let journal = &report.journal;
  assert_eq!(journal.today_files.count, 2);
  assert_eq!(journal.waiting, ["2026-01-02"]);
  assert_eq!(journal.archived.days, 2);
  assert_eq!(journal.archived.bytes, 150);
  assert_eq!(journal.backfill_waiting.count, 1);
  assert_eq!(journal.growth, None);
  assert_eq!(report.index.map(|(bytes, _)| bytes), Some(1));

  let lines = journal_lines(&report);
  assert!(lines[0].starts_with(&format!("today (UTC {today})  2 files")));
  assert_eq!(
    lines[1],
    "waiting for compact  1 days (UTC) from 2026-01-02  5 B"
  );
  assert_eq!(
    lines[2],
    "archived  2 days (UTC), 2026-01-01 to 2026-01-03  150 B"
  );
  let all = text(&report).join("\n");
  assert!(all.contains("STALE: last heard 1m00s ago"), "{all}");
  assert!(all.contains("healthy for 58m20s"), "{all}");

  // A second look sees today's growth.
  dir.write(
    format!("data/journal/live/{today}/a-1.journal"),
    "0".repeat(5000),
  );
  // The frame being written was finished, and the connection closed.
  dir.write(
    format!("data/journal/live/{today}/a-3.journal"),
    "journal dgg.0.3\u{1e}\n1 a-3 1 open wss://x\u{1e}\n2 a-3 1 frame MSG {}\u{1e}\n\
     3 a-3 1 frame NAMES {}\u{1e}\n4 a-3 1 frame JOIN {}\u{1e}\n5 a-3 1 close bye\u{1e}\n",
  );
  let report = watcher.gather(&paths).unwrap();
  assert!(report.journal.growth.is_some_and(|rate| rate > 0.0));
  assert_eq!(report.captures[2].frames, 3);
  assert!(text(&report).join("\n").contains("3 frames"));
}

#[test]
fn tails_logs() {
  let dir = TempDir::new("deploy-status-tail");
  let lines: String = (0..10).map(|n| format!("line {n}\n")).collect();
  dir.write("x.log", &lines);
  assert_eq!(
    tail(&dir.path().join("x.log"), 3).unwrap(),
    ["line 7", "line 8", "line 9"]
  );
  assert_eq!(tail(&dir.path().join("x.log"), 50).unwrap().len(), 10);
  // Past 64 KB, a cut first line is dropped.
  dir.write("big.log", format!("{}\nlast\n", "x".repeat(70_000)));
  assert_eq!(tail(&dir.path().join("big.log"), 5).unwrap(), ["last"]);
}

#[test]
fn draws_the_watch_screen() {
  use ::ratatui::{Terminal, backend::TestBackend};

  let dir = TempDir::new("deploy-status-watch");
  let now = now_ms();
  dir.write(
    "data/journal/control/a-1.status",
    status("a-1", now - 60_000, now, Some(now - 50_000)),
  );
  let report = Watcher::new()
    .unwrap()
    .gather(&Paths::under(dir.to_path_buf()))
    .unwrap();
  let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
  let pane = ::deploy::status::watch::Pane {
    title: "capture.log".into(),
    header: None,
    lines: vec!["2026-11-09 12:00:00.000Z capturing as a-1".to_owned()],
    empty: "(empty)",
  };
  terminal
    .draw(|frame| ::deploy::status::watch::draw(frame, &report, &pane))
    .unwrap();
  let screen: String = terminal
    .backend()
    .buffer()
    .content()
    .chunks(100)
    .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>() + "\n")
    .collect();
  for expected in [
    " programs ",
    " captures ",
    "healthy for 50.0s",
    "0 frames",
    " journal ",
    " capture.log ",
    "capturing as a-1",
    "l logs  r records  space redraw  e open data  q quit | 8 kick",
    " 1 supervisor  2 migrate  3 capture",
  ] {
    assert!(screen.contains(expected), "{expected:?} not in\n{screen}");
  }
}

#[test]
fn the_watch_keys_take_more_lines_when_narrow() {
  use ::deploy::status::watch::keys;

  let wide = keys(200);
  assert_eq!(wide.len(), 1, "{wide:?}");
  assert!(wide[0].contains("q quit | 1 supervisor"), "{wide:?}");
  assert!(wide[0].ends_with("7 sync | 8 kick"), "{wide:?}");
  let narrow = keys(80);
  assert_eq!(narrow.len(), 2, "{narrow:?}");
  assert!(narrow.iter().all(|line| line.len() <= 80), "{narrow:?}");
  // Kick's group goes on the first line with room.
  assert!(narrow[0].ends_with("q quit | 8 kick"), "{narrow:?}");
  assert!(narrow[1].starts_with(" 1 supervisor"), "{narrow:?}");
  assert!(narrow[1].ends_with("7 sync"), "{narrow:?}");
}

#[test]
fn reports_the_peers() {
  let dir = TempDir::new("deploy-status-peer");
  let paths = Paths::under(dir.to_path_buf());
  let mut watcher = Watcher::new().unwrap();
  // No peers: nothing about syncing.
  let report = watcher.gather(&paths).unwrap();
  assert_eq!(report.sync, None);
  assert!(!text(&report).join("\n").contains("sync"));

  dir.write(
    "peer.env",
    "LOGGER_PEER_URL=http://peer:8788\nLOGGER_PEER_TOKEN=secret\n",
  );
  dir.write("nodes", "do https://do:8788\nme push\n");
  dir.write("mesh.env", "LOGGER_NODE=me\nLOGGER_PEER_TOKEN=mesh\n");
  let lines = journal_lines(&watcher.gather(&paths).unwrap());
  assert_eq!(
    lines[lines.len() - 2..],
    [
      "sync from do (https://do:8788)  last: no round yet",
      "sync from peer (http://peer:8788)  last: no round yet",
    ]
  );
  // Each peer's last line in the log, without its name.
  dir.write(
    "logs/sync.log",
    "2026-11-10 12:00:00.000Z peer: 3 files listed, 1 updated\n\
     2026-11-10 12:00:01.000Z do: 5 files listed, 0 updated\n\
     2026-11-10 12:00:02.000Z sync: exited\n",
  );
  let all = text(&watcher.gather(&paths).unwrap()).join("\n");
  assert!(
    all.contains("(https://do:8788)  last: 2026-11-10 12:00:01.000Z 5 files"),
    "{all}"
  );
  assert!(
    all.contains("(http://peer:8788)  last: 2026-11-10 12:00:00.000Z 3 files"),
    "{all}"
  );
  assert!(!all.contains("secret") && !all.contains("mesh "), "{all}");
  // A broken file says so, and the rest are still shown.
  dir.write("peer.env", "LOGGER_PEER_URL=http://peer:8788\n");
  let sync = watcher.gather(&paths).unwrap().sync.unwrap();
  assert_eq!(sync.peers.len(), 1);
  assert!(sync.problems[0].contains("peer.env"), "{:?}", sync.problems);
}

#[test]
fn tails_a_captures_frames() {
  let dir = TempDir::new("deploy-status-frames");
  let root = dir.path().join("journal");
  assert!(capture_tail(&root, "a-1", 10).unwrap().is_empty());
  // 2026-11-09 12:00:00.000 UTC, then on.
  let at = 1_794_225_600_000_i64;
  let mut text =
    format!("journal dgg.0.3\u{1e}\n{at} a-1 1 open wss://x\u{1e}\n");
  for n in 1..=3 {
    text += &format!("{} a-1 1 frame MSG {{\"n\":{n}}}\u{1e}\n", at + n);
  }
  text += &format!(
    "{} a-1 1 close gone\u{1e}\n{} a-1 2 frame PI",
    at + 9,
    at + 10
  );
  dir.write("journal/live/2026-11-09/a-1.journal", &text);
  dir.write(
    "journal/live/2026-11-09/a-2.journal",
    "journal dgg.0.3\u{1e}\n",
  );
  assert_eq!(
    capture_tail(&root, "a-1", 3).unwrap(),
    [
      "12:00:00.002Z #1 MSG {\"n\":2}",
      "12:00:00.003Z #1 MSG {\"n\":3}",
      "12:00:00.009Z #1 (disconnected: gone)",
    ]
  );
  assert_eq!(
    capture_tail(&root, "a-1", 10).unwrap()[0],
    "12:00:00.000Z #1 (connected to wss://x)"
  );
  // A newer day's file is the one shown.
  dir.write(
    "journal/live/2026-11-10/a-1.journal",
    format!(
      "journal dgg.0.3\u{1e}\n{} a-1 2 frame PING {{}}\u{1e}\n",
      at + 86_400_000
    ),
  );
  assert_eq!(
    capture_tail(&root, "a-1", 10).unwrap(),
    ["12:00:00.000Z #2 PING {}"]
  );
}

#[test]
fn reports_gaps_without_writing_the_index() {
  let dir = TempDir::new("deploy-status-gaps");
  let index = dir.path().join("index.sqlite");
  assert_eq!(gaps(&index), Err("no index yet".into()));
  // Connected 1 s to 2 s, then again from 5 s: one 3 s gap, not filled.
  dir.write(
    "journal/live/1970-01-01/a.journal",
    "journal dgg.0.3\u{1e}\n1000 a 1 open wss://x\u{1e}\n2000 a 1 close reset\u{1e}\n\
     5000 a 2 open wss://x\u{1e}\n6000 a 2 close shutdown\u{1e}\n",
  );
  let mut db = ::logger::index::open(&index).unwrap();
  ::logger::index::index(&mut db, &dir.path().join("journal")).unwrap();
  drop(db);
  let (summary, lines) = gaps(&index).unwrap().unwrap();
  assert!(summary.ends_with("1 gaps, 3.0s in all; 1 not filled, 3.0s"));
  assert_eq!(
    lines,
    [
      "1970-01-01 00:00:02.000Z +3.0s  NOT covered: no HISTORY, 0 messages recovered",
      "  resumed by a#2",
    ]
  );
}

#[test]
fn reports_kicks_captures_and_channels() {
  use ::logger_journal::Variant;

  let dir = TempDir::new("deploy-status-kick");
  let now = now_ms();
  let today = utc_day(now);
  let paths = Paths::under(dir.to_path_buf());
  // Without a list or a journal, nothing's said of Kick.
  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  assert!(::deploy::status::kick_lines(&report).is_empty());
  assert!(!text(&report).contains(&"KICK".to_owned()));

  dir.write("kick-channels", "one 2 1\ntwo 4 3\n");
  let kick_status = |heartbeat: i64| {
    let status = ::logger::control::Status {
      instance: "k-1".into(),
      started: now - 60_000,
      heartbeat,
      healthy_since: Some(now - 50_000),
      build: None,
    };
    status.encode_as(Variant::Kick)
  };
  // The same instance in each channel; dgg's status isn't Kick's.
  dir.write("data/journal/kick/one/control/k-1.status", kick_status(now));
  dir.write(
    "data/journal/kick/two/control/k-1.status",
    kick_status(now - 500),
  );
  dir.write(
    "data/journal/kick/two/control/d-1.status",
    status("d-1", now, now, None),
  );
  dir.write(
    format!("data/journal/kick/one/live/{today}/k-1.journal"),
    "12345",
  );
  dir.write("data/journal/kick/gone/live/2026-01-01/k-0.journal", "1");

  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  let kick = &report.kick;
  let instances: Vec<_> = kick
    .captures
    .iter()
    .map(|capture| (capture.instance.as_str(), capture.health))
    .collect();
  assert_eq!(instances, [("k-1", Health::Healthy(now - 50_000))]);
  let channels: Vec<_> = kick
    .channels
    .iter()
    .map(|c| (c.slug.as_str(), c.listed, c.today_files.bytes))
    .collect();
  assert_eq!(
    channels,
    [("gone", false, 0), ("one", true, 5), ("two", true, 0)]
  );
  // Not dgg's captures.
  assert!(report.captures.is_empty());
  let lines = text(&report);
  let kick = lines.iter().position(|line| line == "KICK").unwrap();
  assert!(
    lines[kick + 1].contains("k-1") && lines[kick + 1].contains("healthy")
  );
  assert!(
    lines[kick + 2].contains("gone")
      && lines[kick + 2].contains("(not listed)")
  );
  assert!(
    lines[kick + 3].contains("one") && lines[kick + 3].contains("last write")
  );
  assert!(
    lines[kick + 4].contains("two")
      && lines[kick + 4].contains("nothing today")
  );

  dir.write("kick-channels", "one\n");
  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  let problem = report.kick.problem.as_deref().unwrap();
  assert!(problem.contains("line 1"), "{problem}");
}

#[test]
fn merges_the_logs_in_time_order() {
  let dir = TempDir::new("deploy-status-all-logs");
  dir.write(
    "capture.log",
    "2026-11-24 10:00:01.000Z a-1: capturing\n\
     2026-11-24 10:00:03.000Z a-1: handed off to a-2\n",
  );
  dir.write(
    "supervisor.log",
    "2026-11-24 10:00:00.000Z started\n\
     2026-11-24 10:00:02.000Z capture: started, pid 1\n\
     a line without a time\n",
  );
  let all = all_logs(dir.path(), 500);
  assert_eq!(
    all,
    [
      "2026-11-24 10:00:00.000Z supervisor   started",
      "2026-11-24 10:00:01.000Z capture      a-1: capturing",
      "2026-11-24 10:00:02.000Z supervisor   capture: started, pid 1",
      "                         supervisor   a line without a time",
      "2026-11-24 10:00:03.000Z capture      a-1: handed off to a-2",
    ]
  );
  assert_eq!(all_logs(dir.path(), 2).len(), 2);
}
