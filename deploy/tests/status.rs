//! The status report, against a made-up data directory.

use {
  ::deploy::{
    log::now_ms,
    paths::Paths,
    status::{Health, Watcher, format, journal_lines, tail, text},
  },
  ::logger_journal::layout::utc_day,
  ::std::{fs, path::PathBuf},
};

struct TempDir(PathBuf);

impl TempDir {
  fn new(name: &str) -> Self {
    let path = ::std::env::temp_dir()
      .join(format!("deploy-status-{name}-{}", ::std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    Self(path)
  }

  fn write(&self, path: &str, contents: &str) {
    let path = self.0.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

fn status(
  instance: &str,
  started: i64,
  heartbeat: i64,
  healthy: Option<i64>,
) -> String {
  let mut text = format!(
    "capture-status 0.0\ninstance {instance}\nstarted {started}\nheartbeat {heartbeat}\n"
  );
  if let Some(since) = healthy {
    text += &format!("healthy-since {since}\n");
  }
  text
}

#[test]
fn reports_captures_and_the_journal() {
  let dir = TempDir::new("report");
  let now = now_ms();
  let today = utc_day(now);
  let control = "data/journal/control";
  dir.write(
    &format!("{control}/a-1.status"),
    &status("a-1", now - 3_600_000, now - 500, Some(now - 3_500_000)),
  );
  dir.write(
    &format!("{control}/a-2.status"),
    &status("a-2", now - 120_000, now - 60_000, None),
  );
  dir.write(
    &format!("{control}/a-3.status"),
    &status("a-3", now - 2000, now - 100, None),
  );
  // Two frames, a connection opening, and a frame still being written.
  dir.write(
    &format!("data/journal/live/{today}/a-3.journal"),
    "journal 0.1\n1 a-3 1 open wss://x\n2 a-3 1 frame MSG {}\n\
     3 a-3 1 frame NAMES {}\n4 a-3 1 frame JO",
  );
  dir.write(
    &format!("data/journal/live/{today}/a-1.journal"),
    "0123456789",
  );
  dir.write("data/journal/live/2026-01-02/a-1.journal", "01234");
  dir.write("data/journal/backfill/live/2026-01-01/b.journal", "012");
  dir.write(
    "data/journal/archive/2026/01/manifest",
    &format!(
      "journal-archive 0.0\n2026-01-01 100 1000 10 {0}\n2026-01-03 50 500 5 {0}\n",
      "0".repeat(64)
    ),
  );
  dir.write("data/index.sqlite", "x");

  let paths = Paths::under(dir.0.clone());
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
  assert_eq!(lines[1], "waiting for compact  1 days from 2026-01-02  5 B");
  assert_eq!(
    lines[2],
    "archived  2 days, 2026-01-01 to 2026-01-03  150 B"
  );
  let all = text(&report).join("\n");
  assert!(all.contains("STALE: last heard 1m 0s ago"), "{all}");
  assert!(all.contains("healthy for 58m 20s"), "{all}");

  // A second look sees today's growth.
  dir.write(
    &format!("data/journal/live/{today}/a-1.journal"),
    &"0".repeat(5000),
  );
  // The frame being written was finished, and the connection closed.
  dir.write(
    &format!("data/journal/live/{today}/a-3.journal"),
    "journal 0.1\n1 a-3 1 open wss://x\n2 a-3 1 frame MSG {}\n\
     3 a-3 1 frame NAMES {}\n4 a-3 1 frame JOIN {}\n5 a-3 1 close bye\n",
  );
  let report = watcher.gather(&paths).unwrap();
  assert!(report.journal.growth.is_some_and(|rate| rate > 0.0));
  assert_eq!(report.captures[2].frames, 3);
  assert!(text(&report).join("\n").contains("3 frames"));
}

#[test]
fn tails_logs() {
  let dir = TempDir::new("tail");
  let lines: String = (0..10).map(|n| format!("line {n}\n")).collect();
  dir.write("x.log", &lines);
  assert_eq!(
    tail(&dir.0.join("x.log"), 3).unwrap(),
    ["line 7", "line 8", "line 9"]
  );
  assert_eq!(tail(&dir.0.join("x.log"), 50).unwrap().len(), 10);
  // Past 64 KB, a cut first line is dropped.
  dir.write("big.log", &format!("{}\nlast\n", "x".repeat(70_000)));
  assert_eq!(tail(&dir.0.join("big.log"), 5).unwrap(), ["last"]);
}

#[test]
fn formats_ages_and_sizes() {
  assert_eq!(format::age(42_000), "42s");
  assert_eq!(format::age(185_000), "3m 5s");
  assert_eq!(format::age(7_800_000), "2h 10m");
  assert_eq!(format::age(273_600_000), "3d 4h");
  assert_eq!(format::size(512.0), "512 B");
  assert_eq!(format::size(1536.0), "1.5 KB");
  assert_eq!(format::size(200.0 * 1024.0 * 1024.0), "200 MB");
  assert_eq!(format::count(987), "987");
  assert_eq!(format::count(14_987), "14,987");
  assert_eq!(format::count(1_204_331), "1,204,331");
}

#[test]
fn draws_the_watch_screen() {
  use ::ratatui::{Terminal, backend::TestBackend};

  let dir = TempDir::new("watch");
  let now = now_ms();
  dir.write(
    "data/journal/control/a-1.status",
    &status("a-1", now - 60_000, now, Some(now - 50_000)),
  );
  let report = Watcher::new()
    .unwrap()
    .gather(&Paths::under(dir.0.clone()))
    .unwrap();
  let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
  let log = vec!["2026-11-09 12:00:00.000Z capturing as a-1".to_owned()];
  terminal
    .draw(|frame| {
      ::deploy::status::watch::draw(frame, &report, "capture", &log)
    })
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
    "healthy for 50s",
    " journal ",
    " capture.log ",
    "capturing as a-1",
    "1 capture  2 index",
  ] {
    assert!(screen.contains(expected), "{expected:?} not in\n{screen}");
  }
}

#[test]
fn reports_the_peer() {
  let dir = TempDir::new("peer");
  let paths = Paths::under(dir.0.clone());
  let mut watcher = Watcher::new().unwrap();
  // No peer file: nothing about syncing.
  let report = watcher.gather(&paths).unwrap();
  assert_eq!(report.sync, None);
  assert!(
    !text(&report)
      .join(
        "
"
      )
      .contains("sync")
  );

  dir.write(
    "peer.env",
    "LOGGER_PEER_URL=http://peer:8788
LOGGER_PEER_TOKEN=secret
",
  );
  let lines = journal_lines(&watcher.gather(&paths).unwrap());
  assert_eq!(
    lines.last().unwrap(),
    "sync from http://peer:8788  last: no round yet"
  );
  dir.write(
    "logs/sync.log",
    "2026-11-10 12:00:00.000Z 3 files listed, 1 updated, 10 bytes pulled
",
  );
  let all = text(&watcher.gather(&paths).unwrap()).join(
    "
",
  );
  assert!(
    all.contains("last: 2026-11-10 12:00:00.000Z 3 files"),
    "{all}"
  );
  assert!(!all.contains("secret"), "{all}");
  // A broken file says so.
  dir.write(
    "peer.env",
    "LOGGER_PEER_URL=http://peer:8788
",
  );
  let report = watcher.gather(&paths).unwrap();
  assert!(report.sync.unwrap().peer.is_err());
}
