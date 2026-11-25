//! The status report, against a made-up data directory.

use {
  ::common::temp_dir::TempDir,
  ::common::time::{now_ms, utc_day},
  ::deploy::{
    paths::Paths,
    status::{
      Health, LOGS, Log, Watcher, Which, capture_logs, capture_tail, gaps,
      journal_lines, merge_logs, tail, text,
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
    listening: None,
  };
  status.encode()
}

#[test]
fn reports_captures_and_the_journal() {
  let dir = TempDir::new("deploy-status-report");
  let now = now_ms();
  let today = utc_day(now);
  let control = "data/journal/dgg/destiny.gg/control";
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
    format!("data/journal/dgg/destiny.gg/live/{today}/a-3.journal"),
    "journal dgg.0.3\u{1e}\n1 a-3 1 open wss://x\u{1e}\n2 a-3 1 frame MSG {}\u{1e}\n\
     3 a-3 1 frame NAMES {}\u{1e}\n4 a-3 1 frame JO",
  );
  dir.write(
    format!("data/journal/dgg/destiny.gg/live/{today}/a-1.journal"),
    "0123456789",
  );
  dir.write(
    "data/journal/dgg/destiny.gg/live/2026-01-02/a-1.journal",
    "01234",
  );
  dir.write(
    "data/journal/dgg/destiny.gg/backfill/live/2026-01-01/b.journal",
    "012",
  );
  dir.write(
    "data/journal/dgg/destiny.gg/archive/2026/01/manifest",
    format!(
      "journal-archive dgg.0.2\u{1e}\n2026-01-01 100 1000 10 {0}\u{1e}\n\
       2026-01-03 50 500 5 {0}\u{1e}\n",
      "0".repeat(64)
    ),
  );
  dir.write("data/index-dgg-destiny.gg.sqlite", "x");

  let paths = Paths::under(dir.to_path_buf());
  let mut watcher = Watcher::new().unwrap();
  let report = watcher.gather(&paths).unwrap();

  let health: Vec<_> = report
    .captures
    .iter()
    .map(|capture| (capture.instance.as_str(), capture.health))
    .collect();
  // The unhealthiest first.
  assert_eq!(
    health,
    [
      ("a-2", Health::Stale(now - 60_000)),
      ("a-3", Health::Connecting),
      ("a-1", Health::Healthy(now - 3_500_000)),
    ]
  );
  let frames: Vec<_> = report.captures.iter().map(|c| c.frames).collect();
  assert_eq!(frames, [0, 2, 0]);
  // Each listens to the channel it's in.
  let listening: Vec<_> = report
    .captures
    .iter()
    .map(|c| c.listening.as_deref())
    .collect();
  assert_eq!(listening, [Some("destiny.gg"); 3]);
  let journal = &report.dgg.channels[0].journal;
  assert_eq!(journal.today_files.count, 2);
  assert_eq!(journal.waiting, ["2026-01-02"]);
  assert_eq!(journal.archived.days, 2);
  assert_eq!(journal.archived.bytes, 150);
  assert_eq!(journal.backfill_waiting.count, 1);
  assert_eq!(journal.growth, None);
  assert_eq!(
    report.dgg.channels[0].index.map(|(bytes, _)| bytes),
    Some(1)
  );

  let lines = journal_lines(&report);
  let first = format!("dgg  destiny.gg{:10} today (UTC {today})  2 files", "");
  assert!(lines[0].starts_with(&first), "{}", lines[0]);
  assert_eq!(
    lines[1],
    "  waiting for compact  1 days (UTC) from 2026-01-02  5 B"
  );
  assert_eq!(
    lines[2],
    "  archived  2 days (UTC), 2026-01-01 to 2026-01-03  150 B"
  );
  let all = text(&report).join("\n");
  assert!(all.contains("STALE: last heard 1m00s ago"), "{all}");
  assert!(all.contains("healthy for 58m20s"), "{all}");

  // A second look sees today's growth.
  dir.write(
    format!("data/journal/dgg/destiny.gg/live/{today}/a-1.journal"),
    "0".repeat(5000),
  );
  // The frame being written was finished, and the connection closed.
  dir.write(
    format!("data/journal/dgg/destiny.gg/live/{today}/a-3.journal"),
    "journal dgg.0.3\u{1e}\n1 a-3 1 open wss://x\u{1e}\n2 a-3 1 frame MSG {}\u{1e}\n\
     3 a-3 1 frame NAMES {}\u{1e}\n4 a-3 1 frame JOIN {}\u{1e}\n5 a-3 1 close bye\u{1e}\n",
  );
  let report = watcher.gather(&paths).unwrap();
  let journal = &report.dgg.channels[0].journal;
  assert!(journal.growth.is_some_and(|rate| rate > 0.0));
  assert_eq!(report.captures[1].frames, 3);
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
    "data/journal/dgg/destiny.gg/control/a-1.status",
    status("a-1", now - 60_000, now, Some(now - 50_000)),
  );
  let mut report = Watcher::new()
    .unwrap()
    .gather(&Paths::under(dir.to_path_buf()))
    .unwrap();
  // The programs running here aren't the test's: an install on this
  // machine would push the log off the screen.
  report.programs.clear();
  let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
  let pane = ::deploy::status::watch::Pane {
    title: "dgg-capture.log".into(),
    header: None,
    lines: vec![
      "2026-11-09 12:00:00.000Z capturing as a-1".to_owned(),
      "2026-11-09 12:00:01.000Z ERROR a-1: boom".to_owned(),
    ],
    empty: "(empty)",
  };
  terminal
    .draw(|frame| {
      let mut boxes = ::deploy::status::watch::Boxes::default();
      ::deploy::status::watch::draw(frame, &report, &pane, &mut boxes)
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
    "healthy for 50.0s",
    "a-1                            destiny.gg  up 1m00s",
    " journal ",
    " dgg-capture.log ",
    "capturing as a-1",
    " l logs  r records  f channel  space redraw  e open data  q quit ",
    " 1 supervisor  2 migrate  3 capture  4 compact  5 index  6 gaps  7 sync  \
     8 serve",
  ] {
    assert!(screen.contains(expected), "{expected:?} not in\n{screen}");
  }
  // An error's message is red.
  let row = screen
    .lines()
    .position(|row| row.contains("a-1: boom"))
    .unwrap();
  let column = screen.lines().nth(row).unwrap().find("a-1: boom").unwrap();
  let cell = &terminal.backend().buffer()[(column as u16, row as u16)];
  assert_eq!(cell.fg, ::ratatui::style::Color::Red);
}

#[test]
fn scrolls_and_minimizes_the_watch_boxes() {
  use {
    ::deploy::status::watch::{Boxes, Pane, draw},
    ::ratatui::{Terminal, backend::TestBackend, style::Color},
  };

  let dir = TempDir::new("deploy-status-boxes");
  let now = now_ms();
  for n in 0..10 {
    dir.write(
      format!("data/journal/dgg/destiny.gg/control/c-{n}.status"),
      status(&format!("c-{n}"), now - 10_000 + n, now, Some(now - 5000)),
    );
  }
  let mut report = Watcher::new()
    .unwrap()
    .gather(&Paths::under(dir.to_path_buf()))
    .unwrap();
  report.programs.clear();
  let pane = Pane {
    title: "the page".into(),
    header: None,
    lines: (0..50).map(|n| format!("line {n}")).collect(),
    empty: "(empty)",
  };
  let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
  let mut boxes = Boxes::default();
  let mut screen = |boxes: &mut Boxes| {
    terminal
      .draw(|frame| draw(frame, &report, &pane, boxes))
      .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows: Vec<String> = buffer
      .content()
      .chunks(100)
      .map(|row| row.iter().map(|cell| cell.symbol()).collect())
      .collect();
    (rows, buffer)
  };
  let shown =
    |rows: &[String]| rows.iter().filter(|row| row.contains(" c-")).count();

  // Too many captures for the room: the box says which it shows.
  let (rows, _) = screen(&mut boxes);
  let all = rows.join("\n");
  assert!(all.contains(" captures (1-"), "{all}");
  assert!(all.contains(" of 10) "), "{all}");
  let fit = shown(&rows);
  assert!(fit < 10, "{all}");
  // The page shows its end.
  assert!(all.contains("line 49"), "{all}");

  // Scrolled, and focused: its border highlighted.
  boxes.focus = 1;
  boxes.scroll[1] = 2;
  let (rows, buffer) = screen(&mut boxes);
  let all = rows.join("\n");
  assert!(all.contains(" captures (3-"), "{all}");
  assert!(
    !all.contains("dgg  c-0 ") && all.contains("dgg  c-2 "),
    "{all}"
  );
  let row = rows
    .iter()
    .position(|row| row.contains(" captures ("))
    .unwrap();
  assert_eq!(buffer[(0, row as u16)].fg, Color::Cyan);

  // The page scrolled back, then minimized: the captures take its room.
  boxes.scroll[3] = 5;
  let (rows, _) = screen(&mut boxes);
  let all = rows.join("\n");
  assert!(all.contains("(5 lines back from the end)"), "{all}");
  assert!(all.contains("line 44") && !all.contains("line 45"), "{all}");
  boxes.minimized[3] = true;
  let (rows, _) = screen(&mut boxes);
  let all = rows.join("\n");
  assert!(all.contains(" the page (minimized) "), "{all}");
  assert!(shown(&rows) > fit, "{all}");

  // Minimized captures: just their title line.
  boxes.minimized[1] = true;
  let (rows, _) = screen(&mut boxes);
  let all = rows.join("\n");
  assert!(all.contains(" captures (minimized, 10 lines) "), "{all}");
  assert_eq!(shown(&rows), 0, "{all}");
}

#[test]
fn the_watch_keys_take_more_lines_when_narrow() {
  use ::deploy::status::watch::keys;

  let wide = keys(200);
  assert_eq!(wide.len(), 1, "{wide:?}");
  assert!(wide[0].contains("q quit | 1 supervisor"), "{wide:?}");
  assert!(wide[0].contains("8 serve | tab focus"), "{wide:?}");
  assert!(wide[0].ends_with("m minimize"), "{wide:?}");
  let narrow = keys(80);
  assert_eq!(narrow.len(), 3, "{narrow:?}");
  assert!(narrow.iter().all(|line| line.len() <= 80), "{narrow:?}");
  assert!(narrow[0].ends_with("q quit"), "{narrow:?}");
  assert!(narrow[1].starts_with(" 1 supervisor"), "{narrow:?}");
  assert!(narrow[1].ends_with("8 serve"), "{narrow:?}");
  assert!(narrow[2].ends_with("m minimize"), "{narrow:?}");
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
    "install.env.json",
    r#"{"mesh": {"node": "me", "token": "mesh"},
        "peers": {"peer": {"url": "http://peer:8788", "token": "secret"}}}"#,
  );
  dir.write("nodes", "do https://do:8788\nme push\n");
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
  // A file from before the config says so, and the peers are still shown.
  dir.write("peer.env", "LOGGER_PEER_URL=http://peer:8788\n");
  let sync = watcher.gather(&paths).unwrap().sync.unwrap();
  assert_eq!(sync.peers.len(), 2);
  assert!(sync.problems[0].contains("peer.env"), "{:?}", sync.problems);
  // A broken config says so.
  dir.write("install.env.json", "{");
  let sync = watcher.gather(&paths).unwrap().sync.unwrap();
  assert!(
    sync.problems[0].contains("install.env.json"),
    "{:?}",
    sync.problems
  );
}

#[test]
fn tails_a_captures_frames() {
  let dir = TempDir::new("deploy-status-frames");
  let root = dir.path().join("journal");
  let a1 = Which {
    kick: false,
    channel: "destiny.gg".into(),
    instance: "a-1".into(),
  };
  assert!(capture_tail(&root, &a1, 10).unwrap().is_empty());
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
  dir.write("journal/dgg/destiny.gg/live/2026-11-09/a-1.journal", &text);
  dir.write(
    "journal/dgg/destiny.gg/live/2026-11-09/a-2.journal",
    "journal dgg.0.3\u{1e}\n",
  );
  assert_eq!(
    capture_tail(&root, &a1, 3).unwrap(),
    [
      "12:00:00.002Z #1 MSG {\"n\":2}",
      "12:00:00.003Z #1 MSG {\"n\":3}",
      "12:00:00.009Z #1 (disconnected: gone)",
    ]
  );
  assert_eq!(
    capture_tail(&root, &a1, 10).unwrap()[0],
    "12:00:00.000Z #1 (connected to wss://x)"
  );
  // A newer day's file is the one shown.
  dir.write(
    "journal/dgg/destiny.gg/live/2026-11-10/a-1.journal",
    format!(
      "journal dgg.0.3\u{1e}\n{} a-1 2 frame PING {{}}\u{1e}\n",
      at + 86_400_000
    ),
  );
  assert_eq!(
    capture_tail(&root, &a1, 10).unwrap(),
    ["12:00:00.000Z #2 PING {}"]
  );

  // A Kick channel's capture, of the same instance name.
  dir.write(
    "journal/kick/one/live/2026-11-09/a-1.journal",
    format!(
      "journal kick.0.1 ; channel=one\u{1e}\n{at} a-1 1 frame {{}}\u{1e}\n\
       {} a-1 1 history - {{\"data\":[]}}\u{1e}\n",
      at + 1
    ),
  );
  let kick = Which {
    kick: true,
    channel: "one".into(),
    instance: "a-1".into(),
  };
  assert_eq!(
    capture_tail(&root, &kick, 10).unwrap(),
    [
      "12:00:00.000Z #1 {}",
      "12:00:00.001Z #1 (history page, 11 bytes)"
    ]
  );
}

#[test]
fn reports_gaps_without_writing_the_index() {
  let dir = TempDir::new("deploy-status-gaps");
  let index = dir.path().join("index-dgg-destiny.gg.sqlite");
  assert_eq!(gaps(&index), Err("no index yet".into()));
  // Connected 1 s to 2 s, then again from 5 s: one 3 s gap, not filled.
  dir.write(
    "journal/dgg/destiny.gg/live/1970-01-01/a.journal",
    "journal dgg.0.3\u{1e}\n1000 a 1 open wss://x\u{1e}\n2000 a 1 close reset\u{1e}\n\
     5000 a 2 open wss://x\u{1e}\n6000 a 2 close shutdown\u{1e}\n",
  );
  let mut db = ::logger::dgg::index::open(&index).unwrap();
  ::logger::dgg::index::index(
    &mut db,
    &dir.path().join("journal/dgg/destiny.gg"),
  )
  .unwrap();
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
  assert!(!text(&report).join("\n").contains("kick "));

  dir.write("kick-channels", "one 2 1\ntwo 4 3\n");
  let kick_status = |instance: &str, heartbeat: i64| {
    let status = ::logger::control::Status {
      instance: instance.into(),
      started: now - 60_000,
      heartbeat,
      healthy_since: Some(now - 50_000),
      build: None,
      listening: None,
    };
    status.encode_as(Variant::Kick)
  };
  // A capture per channel; dgg's status isn't Kick's.
  dir.write(
    "data/journal/kick/one/control/k-1.status",
    kick_status("k-1", now),
  );
  dir.write(
    "data/journal/kick/two/control/k-2.status",
    kick_status("k-2", now - 500),
  );
  dir.write(
    "data/journal/kick/two/control/d-1.status",
    status("d-1", now, now, None),
  );
  // A connection opening and two frames.
  dir.write(
    format!("data/journal/kick/one/live/{today}/k-1.journal"),
    "journal kick.0.1 ; channel=one\u{1e}\n1 k-1 1 open wss://k\u{1e}\n\
     2 k-1 1 frame {}\u{1e}\n3 k-1 1 frame {}\u{1e}\n",
  );
  dir.write("data/journal/kick/gone/live/2026-01-01/k-0.journal", "1");

  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  let captures: Vec<_> = report
    .captures
    .iter()
    .map(|capture| {
      (
        capture.instance.as_str(),
        capture.listening.as_deref(),
        capture.kick,
        capture.health,
        capture.frames,
      )
    })
    .collect();
  let healthy = Health::Healthy(now - 50_000);
  assert_eq!(
    captures,
    [
      ("k-1", Some("one"), true, healthy, 2),
      ("k-2", Some("two"), true, healthy, 0),
    ]
  );
  let channels: Vec<_> = report
    .kick
    .channels
    .iter()
    .map(|c| (c.slug.as_str(), c.listed, c.today_files.bytes))
    .collect();
  let one = channels[1].2;
  assert!(one > 0);
  assert_eq!(
    channels,
    [("gone", false, 0), ("one", true, one), ("two", true, 0)]
  );
  // Kick's captures with dgg's, its channels with the journal.
  let lines = text(&report);
  let captures = lines.iter().position(|line| line == "CAPTURES").unwrap();
  let k1 = &lines[captures + 1];
  assert!(k1.contains("k-1") && k1.contains(" one ") && k1.contains("healthy"));
  assert!(lines[captures + 2].contains("k-2"));
  let journal = lines.iter().position(|line| line == "JOURNAL").unwrap();
  let journal = &lines[journal..];
  let line = |slug: &str| {
    journal
      .iter()
      .find(|line| line.contains(&format!("kick {slug} ")))
      .unwrap()
  };
  assert!(line("gone").contains("(not listed)"));
  assert!(line("one").contains("last write"));
  assert!(line("two").contains("nothing today"));
  assert!(!lines.contains(&"KICK".to_owned()));

  dir.write("kick-channels", "one\n");
  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  let problem = report.kick.problem.as_deref().unwrap();
  assert!(problem.contains("line 1"), "{problem}");
}

#[test]
fn reports_each_dgg_channel() {
  let dir = TempDir::new("deploy-status-dgg-channels");
  let now = now_ms();
  let today = utc_day(now);
  let paths = Paths::under(dir.to_path_buf());
  dir.write(
    "dgg-channels",
    "two.dev wss://two.example/ws -\ndestiny.gg wss://x/ws -\n",
  );
  // A capture of each listed channel; one only journaled, not listed.
  dir.write(
    "data/journal/dgg/two.dev/control/a-1.status",
    status("a-1", now - 60_000, now, Some(now - 50_000)),
  );
  dir.write(
    "data/journal/dgg/destiny.gg/control/a-1.status",
    status("a-1", now - 60_000, now - 500, Some(now - 50_000)),
  );
  dir.write(
    format!("data/journal/dgg/two.dev/live/{today}/a-1.journal"),
    "journal dgg.0.3\u{1e}\n1 a-1 1 open wss://two\u{1e}\n\
     2 a-1 1 frame MSG {}\u{1e}\n3 a-1 1 frame MSG {}\u{1e}\n",
  );
  dir.write("data/journal/dgg/old.chat/live/2026-01-01/a-0.journal", "1");
  dir.write("data/index-dgg-two.dev.sqlite", "12");
  // The old layout's directories aren't channels.
  dir.write("data/journal/dgg/live/2026-01-01/a-0.journal", "1");

  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  let channels: Vec<_> = report
    .dgg
    .channels
    .iter()
    .map(|c| (c.name.as_str(), c.listed, c.index.map(|(bytes, _)| bytes)))
    .collect();
  assert_eq!(
    channels,
    [
      ("two.dev", true, Some(2)),
      ("destiny.gg", true, None),
      ("old.chat", false, None),
    ]
  );
  assert_eq!(report.dgg.problem, None);
  // Each capture's frames are its own channel's.
  let captures: Vec<_> = report
    .captures
    .iter()
    .map(|c| (c.listening.as_deref(), c.frames))
    .collect();
  assert_eq!(captures, [(Some("two.dev"), 2), (Some("destiny.gg"), 0)]);
  let which: Vec<Which> = report.captures.iter().map(|c| c.which()).collect();
  assert_eq!(which[0].channel, "two.dev");
  assert_ne!(which[0], which[1]);

  let lines = journal_lines(&report);
  let at = |name: &str| {
    let start = format!("dgg  {name:<20} today");
    lines
      .iter()
      .position(|line| line.starts_with(&start))
      .unwrap()
  };
  let (two, destiny, old) = (at("two.dev"), at("destiny.gg"), at("old.chat"));
  assert!(two < destiny && destiny < old, "{lines:#?}");
  assert!(lines[two + 1].starts_with("  index  2 B  last write"));
  assert_eq!(lines[destiny + 1], "  index  none yet");
  assert!(lines[old].ends_with("  (not listed)"));
  assert!(lines[old + 1].starts_with("  waiting for compact  1 days"));

  dir.write("dgg-channels", "two.dev\n");
  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  let problem = report.dgg.problem.as_deref().unwrap();
  assert!(problem.contains("line 1"), "{problem}");
  // Still each journaled channel, none listed.
  assert!(report.dgg.channels.iter().all(|c| !c.listed));
  let lines = journal_lines(&report);
  assert!(
    lines
      .iter()
      .any(|line| line.starts_with("dgg channel list  "))
  );
}

#[test]
fn says_what_a_capture_listens_to() {
  let dir = TempDir::new("deploy-status-listening");
  let now = now_ms();
  let status = ::logger::control::Status {
    instance: "a-1".into(),
    started: now - 1000,
    heartbeat: now,
    healthy_since: None,
    build: None,
    listening: Some("wss://chat.example/ws".into()),
  };
  dir.write(
    "data/journal/dgg/destiny.gg/control/a-1.status",
    status.encode(),
  );
  let paths = Paths::under(dir.to_path_buf());
  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  // Its channel's name, not the URL its status says.
  let listening = report.captures[0].listening.as_deref();
  assert_eq!(listening, Some("destiny.gg"));
  let text = text(&report).join("\n");
  assert!(text.contains("a-1") && !text.contains("wss://"), "{text}");
}

#[test]
fn puts_the_busiest_captures_first() {
  let dir = TempDir::new("deploy-status-busiest");
  let now = now_ms();
  let today = utc_day(now);
  for instance in ["a-1", "a-2"] {
    dir.write(
      format!("data/journal/dgg/destiny.gg/control/{instance}.status"),
      status(instance, now - 1000, now, Some(now - 1000)),
    );
  }
  let paths = Paths::under(dir.to_path_buf());
  let mut watcher = Watcher::new().unwrap();
  let instances = |report: &::deploy::status::Report| {
    let captures = report.captures.iter();
    captures.map(|c| c.instance.clone()).collect::<Vec<_>>()
  };
  // Equally quiet, and no rate yet: as they were.
  let report = watcher.gather(&paths).unwrap();
  assert_eq!(instances(&report), ["a-1", "a-2"]);
  assert_eq!(report.captures[0].per_minute, None);

  dir.write(
    format!("data/journal/dgg/destiny.gg/live/{today}/a-2.journal"),
    "journal dgg.0.3\u{1e}\n1 a-2 1 frame MSG {}\u{1e}\n\
     2 a-2 1 frame MSG {}\u{1e}\n",
  );
  ::std::thread::sleep(::std::time::Duration::from_millis(20));
  let mut report = watcher.gather(&paths).unwrap();
  assert_eq!(instances(&report), ["a-2", "a-1"]);
  assert!(report.captures[0].per_minute.is_some_and(|rate| rate > 0.0));
  assert_eq!(report.captures[1].per_minute, Some(0.0));

  // Each prefixed by its service; the programs' columns lined up.
  let program = |name: &str| ::deploy::status::Program {
    name: name.into(),
    pid: 1,
    started: 0,
    memory: 0,
    cpu: 0.0,
    exe: None,
    ours: false,
  };
  report.programs = vec![program("serve"), program("kick-capture longer")];
  let lines = text(&report);
  let captures = lines.iter().position(|line| line == "CAPTURES").unwrap();
  assert!(lines[captures + 1].starts_with("  dgg  a-2 "), "{lines:?}");
  assert!(lines[captures + 2].contains(" 0/min "), "{lines:?}");
  let programs = lines.iter().position(|line| line == "PROGRAMS").unwrap();
  let pid = |line: &String| line.find(" pid ").unwrap();
  assert_eq!(pid(&lines[programs + 1]), pid(&lines[programs + 2]));
}

#[test]
fn merges_the_logs_in_time_order() {
  let dir = TempDir::new("deploy-status-all-logs");
  dir.write(
    "dgg-capture.log",
    "2026-11-24 10:00:01.000Z INFO  a-1: capturing\n\
     2026-11-24 10:00:03.000Z WARN  a-1: control: busy\n",
  );
  // From before levels.
  dir.write(
    "supervisor.log",
    "2026-11-24 10:00:00.000Z started\n\
     2026-11-24 10:00:02.000Z dgg-capture: started, pid 1\n\
     a line without a time\n",
  );
  let every: Vec<Log> = LOGS
    .iter()
    .map(|name| Log {
      name: (*name).into(),
      shown: (*name).into(),
    })
    .collect();
  let all = merge_logs(dir.path(), &every, 500);
  assert_eq!(
    all,
    [
      "2026-11-24 10:00:00.000Z       supervisor   started",
      "2026-11-24 10:00:01.000Z INFO  dgg-capture  a-1: capturing",
      "2026-11-24 10:00:02.000Z       supervisor   dgg-capture: started, pid 1",
      "                               supervisor   a line without a time",
      "2026-11-24 10:00:03.000Z WARN  dgg-capture  a-1: control: busy",
    ]
  );
  assert_eq!(merge_logs(dir.path(), &every, 2).len(), 2);
}

#[test]
fn merges_the_captures_logs() {
  let dir = TempDir::new("deploy-status-capture-logs");
  dir.write("kick-channels", "one 2 1\n");
  dir.write(
    "logs/dgg-capture.log",
    "2026-11-24 10:00:01.000Z INFO  a-1: capturing\n",
  );
  dir.write(
    "logs/kick-capture.one.log",
    "2026-11-24 10:00:02.000Z WARN  a-1: one: dropped\n",
  );
  // Shared by every channel, before each had its own.
  dir.write(
    "logs/kick-capture.log",
    "2026-11-24 10:00:00.000Z INFO  a-0: one: capturing\n",
  );
  dir.write(
    "logs/dgg-capture.destiny.gg.log",
    "2026-11-24 10:00:03.000Z INFO  a-2: capturing\n",
  );
  dir.write("logs/index.log", "2026-11-24 10:00:01.500Z INFO  indexed\n");
  let paths = Paths::under(dir.to_path_buf());
  let report = Watcher::new().unwrap().gather(&paths).unwrap();
  let logs = capture_logs(&report);
  let shown: Vec<_> = logs.iter().map(|log| log.shown.as_str()).collect();
  assert_eq!(
    shown,
    [
      "dgg-capture destiny.gg",
      "dgg-capture",
      "kick-capture one",
      "kick-capture"
    ]
  );
  assert_eq!(
    merge_logs(&paths.logs, &logs, 500),
    [
      "2026-11-24 10:00:00.000Z INFO  kick-capture           a-0: one: \
       capturing",
      "2026-11-24 10:00:01.000Z INFO  dgg-capture            a-1: capturing",
      "2026-11-24 10:00:02.000Z WARN  kick-capture one       a-1: one: dropped",
      "2026-11-24 10:00:03.000Z INFO  dgg-capture destiny.gg a-2: capturing",
    ]
  );
  // Every log has the channels' too.
  let every = ::deploy::status::logs(&report);
  for name in ["dgg-capture.destiny.gg", "kick-capture.one"] {
    assert!(every.iter().any(|log| log.name == name), "{name}");
  }
}
