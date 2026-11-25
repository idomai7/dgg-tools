//! What the logger is doing: its programs, each capture's health (dgg's
//! and each Kick channel's), the journal and index (Kick's channels'
//! too), the peers it syncs from, free disk space and the ends of the
//! logs.
//!
//! [`Watcher::gather`] takes a snapshot ([`Report`]); [`text`] prints one,
//! and [`watch`] redraws one every second.

pub mod watch;

use {
  crate::{
    paths::Paths,
    peer, processes,
    supervise::{dgg_capture_log, kick_capture_log},
  },
  ::common::{
    format,
    time::{now_ms, utc_day, utc_timestamp},
  },
  ::common::{fs::MissingOk, log::Level},
  ::logger::{
    control,
    dgg::{Dgg, channels::DESTINY, index},
    kick::channels,
    service::{Root, Service},
  },
  ::logger_journal::{
    Reader, Record, Variant, archive,
    layout::{channel_root, kick_root, live_file_instance, live_files},
    record::{kick, v0},
  },
  ::std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
  },
  ::sysinfo::{
    Disks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind,
  },
};

/// A capture status counts as stale after this long without a heartbeat,
/// as capture itself judges it.
pub const STALE_MS: i64 = 5000;

/// A capture's frames a minute are counted over about this long, in unix
/// ms: from the last snapshot at least this old.
pub const RATE_OVER: i64 = 60_000;

/// The logs a supervisor writes, in the order they're offered, but for
/// each channel's capture's (see [`logs`]). `dgg-capture` and
/// `kick-capture` are the ones each service's captures wrote before each
/// channel had its own.
pub const LOGS: [&str; 8] = [
  "dgg-capture",
  "index",
  "compact",
  "supervisor",
  "sync",
  "migrate",
  "kick-capture",
  "serve",
];

/// One of the logger's processes.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
  pub name: String,
  pub pid: u32,
  /// Unix seconds.
  pub started: u64,
  pub memory: u64,
  /// Percent of one core since the last snapshot.
  pub cpu: f32,
  pub exe: Option<PathBuf>,
  /// Whether it runs from this install's `bin`.
  pub ours: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
  /// Since unix ms.
  Healthy(i64),
  Connecting,
  /// Last heard from at unix ms.
  Stale(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Capture {
  pub instance: String,
  /// Unix ms.
  pub started: i64,
  pub health: Health,
  /// The build it runs, if it says.
  pub build: Option<String>,
  /// Frames it has captured in its live files (those not yet compacted).
  pub frames: u64,
  /// What it listens to: its dgg channel's name, or the Kick channel's
  /// slug.
  pub listening: Option<String>,
  /// Whether it's Kick's, not dgg's.
  pub kick: bool,
  /// Frames a minute, over about the last [`RATE_OVER`] of snapshots; none
  /// until a second snapshot.
  pub per_minute: Option<f64>,
}

/// A count of files and their size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Files {
  pub count: usize,
  pub bytes: u64,
}

impl Files {
  fn add(&mut self, bytes: u64) {
    self.count += 1;
    self.bytes += bytes;
  }
}

/// Archived days, from the monthly manifests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Archived {
  pub days: usize,
  pub first: Option<String>,
  pub last: Option<String>,
  pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Journal {
  /// `YYYY-MM-DD`, UTC.
  pub today: String,
  pub today_files: Files,
  /// Unix ms of the newest write today.
  pub last_write: Option<i64>,
  /// Bytes a second written today since the last snapshot.
  pub growth: Option<f64>,
  /// Earlier days with live files, waiting to be compacted.
  pub waiting: Vec<String>,
  pub waiting_files: Files,
  pub archived: Archived,
  pub backfill_waiting: Files,
  pub backfill_archived: Archived,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
  /// Unix ms.
  pub now: i64,
  pub paths: Paths,
  pub programs: Vec<Program>,
  /// Unhealthy first (stale, then connecting), then the busiest; dgg's
  /// before Kick's, by channel, when that doesn't tell them apart.
  pub captures: Vec<Capture>,
  pub journal: Journal,
  /// The index's size and last write (unix ms).
  pub index: Option<(u64, i64)>,
  /// Kick's index's, likewise.
  pub kick_index: Option<(u64, i64)>,
  /// Free space on the data directory's disk.
  pub free: Option<u64>,
  pub sync: Option<Sync>,
  pub kick: Kick,
}

/// Kick's channels, listed or journaled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kick {
  pub channels: Vec<KickChannel>,
  /// Why the channel list couldn't be read, if it couldn't.
  pub problem: Option<String>,
}

/// One Kick channel's journal today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KickChannel {
  pub slug: String,
  /// Whether it's in the channel list (or only journaled before).
  pub listed: bool,
  pub today_files: Files,
  /// Unix ms of the newest write today.
  pub last_write: Option<i64>,
}

/// The peers the install syncs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sync {
  pub peers: Vec<Peer>,
  /// What couldn't be read (see [`peer::gather`]).
  pub problems: Vec<String>,
}

/// One peer the install syncs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
  pub name: String,
  pub url: String,
  /// Its last line in `sync.log`: how its last round went.
  pub last: Option<String>,
}

/// Takes snapshots, remembering what it needs from the last ones (CPU use,
/// journal growth, how far it has counted each live file's frames, and
/// each capture's frames over the last minute).
pub struct Watcher {
  system: System,
  previous: Option<(String, i64, u64)>,
  counted: HashMap<PathBuf, Counted>,
  /// Each capture's frames at each snapshot (unix ms).
  frames: HashMap<Which, VecDeque<(i64, u64)>>,
}

/// Which capture one is: its Kick channel (none for dgg's), then its
/// instance. Kick's captures of different channels share instance names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Which {
  pub kick: Option<String>,
  pub instance: String,
}

impl Capture {
  pub fn which(&self) -> Which {
    Which {
      kick: self.kick.then(|| self.listening.clone()).flatten(),
      instance: self.instance.clone(),
    }
  }
}

/// How unhealthy a capture is, the most first.
fn unhealthiest(capture: &Capture) -> u8 {
  match capture.health {
    Health::Stale(_) => 0,
    Health::Connecting => 1,
    Health::Healthy(_) => 2,
  }
}

/// A live file's frames, counted up to `offset`.
#[derive(Clone, Copy, Default)]
struct Counted {
  offset: u64,
  frames: u64,
}

fn modified_ms(metadata: &fs::Metadata) -> Option<i64> {
  let since = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
  Some(since.as_millis() as i64)
}

/// The `YYYY-MM-DD` directory a day file is in.
fn day_of(file: &Path) -> String {
  file
    .parent()
    .and_then(Path::file_name)
    .map(|day| day.to_string_lossy().into_owned())
    .unwrap_or_default()
}

fn archived(archives: &[archive::Archive]) -> Archived {
  Archived {
    days: archives.len(),
    first: archives.first().map(|archive| archive.day.clone()),
    last: archives.last().map(|archive| archive.day.clone()),
    bytes: archives.iter().map(|archive| archive.compressed).sum(),
  }
}

impl Watcher {
  pub fn new() -> io::Result<Self> {
    let system =
      System::new().map_err(|e| io::Error::other(format!("processes: {e}")))?;
    Ok(Self {
      system,
      previous: None,
      counted: HashMap::new(),
      frames: HashMap::new(),
    })
  }

  pub fn gather(&mut self, paths: &Paths) -> io::Result<Report> {
    let now = now_ms();
    let journal_root = paths.data.join("journal");
    let [dgg, backfill] = dgg_roots(&journal_root)?;
    let (kick, kick_captures) = kick(paths, &journal_root, now)?;
    let mut captures = captures(&journal_root, now)?;
    captures.extend(kick_captures);
    let mut files: Vec<(PathBuf, Option<String>)> = dgg
      .live_files()?
      .into_iter()
      .map(|file| (file, None))
      .collect();
    let kick_root = kick_root(&journal_root);
    for channel in &kick.channels {
      let root = channel_root(&kick_root, &channel.slug);
      let theirs = live_files(&root)?.into_iter();
      files.extend(theirs.map(|file| (file, Some(channel.slug.clone()))));
    }
    self.count_frames(&mut captures, files);
    self.rate(&mut captures, now);
    // Stable: ties keep dgg's first, then Kick's by channel.
    captures.sort_by(|a, b| {
      let per_minute = |c: &Capture| c.per_minute.unwrap_or(0.0);
      unhealthiest(a)
        .cmp(&unhealthiest(b))
        .then_with(|| per_minute(b).total_cmp(&per_minute(a)))
    });
    Ok(Report {
      now,
      paths: paths.clone(),
      programs: self.programs(&paths.bin()),
      captures,
      journal: self.journal(&dgg, &backfill, now)?,
      index: size_and_write(&paths.data.join(index::file(DESTINY))),
      kick_index: size_and_write(&paths.data.join(::logger::kick::index::FILE)),
      free: free_space(&paths.data),
      sync: sync(paths),
      kick,
    })
  }

  fn programs(&mut self, bin: &Path) -> Vec<Program> {
    self.system.refresh_processes_specifics(
      ProcessesToUpdate::All,
      true,
      ProcessRefreshKind::nothing()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_memory()
        .with_cpu(),
    );
    let me = ::std::process::id();
    let mut programs: Vec<Program> = self
      .system
      .processes()
      .values()
      .filter(|process| process.pid().as_u32() != me)
      .filter_map(|process| {
        let exe = process.exe().map(Path::to_owned);
        let ours = exe.as_ref().is_some_and(|exe| exe.starts_with(bin));
        let mut name = processes::program(process);
        if let Some(slug) = processes::kick_channel(process) {
          name = format!("{name} {slug}");
        }
        // Any of the logger's programs, or anything run from the bin.
        (name.starts_with("dgg-") || ours).then(|| Program {
          name,
          pid: process.pid().as_u32(),
          started: process.start_time(),
          memory: process.memory(),
          cpu: process.cpu_usage(),
          exe,
          ours,
        })
      })
      .collect();
    programs.sort_by(|a, b| (&a.name, a.started).cmp(&(&b.name, b.started)));
    programs
  }

  /// Adds to `captures` the frames in each one's live files, `files` (each
  /// with its Kick channel, or none for dgg's), read from where the last
  /// count left off.
  fn count_frames(
    &mut self,
    captures: &mut [Capture],
    files: Vec<(PathBuf, Option<String>)>,
  ) {
    // Compacted files are gone; so is their count.
    self
      .counted
      .retain(|file, _| files.iter().any(|(live, _)| live == file));
    for (file, slug) in files {
      let Some((instance, _)) = live_file_instance(&file) else {
        continue;
      };
      let Some(capture) = captures.iter_mut().find(|capture| {
        capture.instance == instance
          && capture.kick == slug.is_some()
          && (slug.is_none() || capture.listening == slug)
      }) else {
        continue;
      };
      let counted = self.counted.entry(file.clone()).or_default();
      // Unreadable now (a newer format, say): counted as it was.
      if let Ok(mut reader) = Reader::open(&file, counted.offset) {
        for entry in reader.by_ref() {
          let Ok(entry) = entry else { continue };
          let frame = match entry.record {
            Record::V0(record) => {
              matches!(record.body, v0::Body::Frame(_) | v0::Body::Binary(_))
            }
            Record::Kick0(record) => matches!(
              record.body,
              kick::v0::Body::Frame(_) | kick::v0::Body::Binary(_)
            ),
          };
          counted.frames += u64::from(frame);
        }
        counted.offset = reader.offset();
      }
      capture.frames += counted.frames;
    }
  }

  /// Sets each capture's frames a minute, from its frames at the last
  /// snapshot at least [`RATE_OVER`] old, or failing one, its oldest.
  fn rate(&mut self, captures: &mut [Capture], now: i64) {
    let mut frames = HashMap::new();
    for capture in captures {
      let key = capture.which();
      let mut seen = self.frames.remove(&key).unwrap_or_default();
      // Fewer than before, as files were compacted away: counted afresh.
      if seen
        .back()
        .is_some_and(|&(_, before)| before > capture.frames)
      {
        seen.clear();
      }
      seen.push_back((now, capture.frames));
      while seen.get(1).is_some_and(|&(at, _)| at <= now - RATE_OVER) {
        seen.pop_front();
      }
      let (at, before) = seen[0];
      if now > at {
        let grown = (capture.frames - before) as f64;
        capture.per_minute = Some(grown * 60_000.0 / (now - at) as f64);
      }
      frames.insert(key, seen);
    }
    // Captures gone are forgotten.
    self.frames = frames;
  }

  fn journal(
    &mut self,
    root: &Root,
    backfill: &Root,
    now: i64,
  ) -> io::Result<Journal> {
    let today = utc_day(now);
    let mut journal = Journal {
      today: today.clone(),
      ..Journal::default()
    };
    let mut waiting = BTreeSet::new();
    for file in root.live_files()? {
      let Ok(metadata) = fs::metadata(&file) else {
        continue;
      };
      let day = day_of(&file);
      if day == today {
        journal.today_files.add(metadata.len());
        journal.last_write = journal.last_write.max(modified_ms(&metadata));
      } else {
        journal.waiting_files.add(metadata.len());
        waiting.insert(day);
      }
    }
    journal.waiting = waiting.into_iter().collect();
    for file in backfill.live_files()? {
      if let Ok(metadata) = fs::metadata(&file) {
        journal.backfill_waiting.add(metadata.len());
      }
    }
    journal.archived = archived(&archive::all(&root.dir)?);
    journal.backfill_archived = archived(&archive::all(&backfill.dir)?);

    let bytes = journal.today_files.bytes;
    if let Some((day, at, before)) = &self.previous
      && *day == today
      && now > *at
    {
      let grown = bytes.saturating_sub(*before) as f64;
      journal.growth = Some(grown * 1000.0 / (now - at) as f64);
    }
    self.previous = Some((today, now, bytes));
    Ok(journal)
  }
}

/// The peers, if there are any (or any problems finding them).
fn sync(paths: &Paths) -> Option<Sync> {
  let found = peer::gather(&paths.root);
  if found.pull.is_empty() && found.problems.is_empty() {
    return None;
  }
  let log = tail(&paths.logs.join("sync.log"), 500).unwrap_or_default();
  let peers = found
    .pull
    .into_iter()
    .map(|named| Peer {
      last: last_round(&log, &named.name),
      name: named.name,
      url: named.peer.url,
    })
    .collect();
  Some(Sync {
    peers,
    problems: found.problems,
  })
}

/// `name`'s last line in `log` (`<date> <time> <name>: ...`), without its
/// name.
fn last_round(log: &[String], name: &str) -> Option<String> {
  let label = format!("{name}: ");
  log.iter().rev().find_map(|line| {
    let mut fields = line.splitn(3, ' ');
    let (date, time, rest) = (fields.next()?, fields.next()?, fields.next()?);
    Some(format!("{date} {time} {}", rest.strip_prefix(&label)?))
  })
}

/// destiny.gg's directory, in the journal directory `journal`: the one dgg
/// channel captured for now.
fn destiny(journal: &Path) -> PathBuf {
  Dgg::channel_dir(&Dgg::dir(journal), DESTINY)
}

/// destiny.gg's journal roots, in the journal directory `journal`: its own,
/// and the backfill journal's.
fn dgg_roots(journal: &Path) -> io::Result<[Root; 2]> {
  Dgg::channel_roots(&destiny(journal), DESTINY)
    .try_into()
    .map_err(|_| io::Error::other("a dgg channel has two journal roots"))
}

/// dgg's captures, from their statuses in destiny.gg's control directory,
/// each listening to destiny.gg.
fn captures(journal: &Path, now: i64) -> io::Result<Vec<Capture>> {
  let mut captures: Vec<Capture> =
    control::read_all(&control::dir(&destiny(journal)))?
      .into_iter()
      .map(|status| Capture {
        listening: Some(DESTINY.into()),
        ..capture(status, now)
      })
      .collect();
  captures.sort_by_key(|capture| capture.started);
  Ok(captures)
}

/// A capture, as `status` has it at `now`.
fn capture(status: control::Status, now: i64) -> Capture {
  Capture {
    health: if now - status.heartbeat > STALE_MS {
      Health::Stale(status.heartbeat)
    } else if let Some(since) = status.healthy_since {
      Health::Healthy(since)
    } else {
      Health::Connecting
    },
    instance: status.instance,
    started: status.started,
    build: status.build,
    frames: 0,
    listening: status.listening,
    kick: false,
    per_minute: None,
  }
}

/// Kick's channels (those in the install's list, and those with a journal
/// root), and their captures, by channel.
fn kick(
  paths: &Paths,
  journal_root: &Path,
  now: i64,
) -> io::Result<(Kick, Vec<Capture>)> {
  let mut report = Kick::default();
  let listed = match channels::load(&paths.root.join(channels::FILE)) {
    Ok(listed) => listed.into_iter().map(|c| c.slug).collect(),
    Err(e) => {
      report.problem = Some(e);
      Vec::new()
    }
  };
  let kick_root = kick_root(journal_root);
  let mut slugs: BTreeSet<String> = listed.iter().cloned().collect();
  if let Some(entries) = fs::read_dir(&kick_root).missing_ok()? {
    for entry in entries {
      let entry = entry?;
      if entry.file_type()?.is_dir() {
        slugs.insert(entry.file_name().to_string_lossy().into_owned());
      }
    }
  }
  let today = utc_day(now);
  let mut captures = Vec::new();
  for slug in slugs {
    let root = channel_root(&kick_root, &slug);
    let mut theirs: Vec<Capture> =
      control::read_all_as(&control::dir(&root), Variant::Kick)?
        .into_iter()
        .map(|status| Capture {
          listening: Some(slug.clone()),
          kick: true,
          ..capture(status, now)
        })
        .collect();
    theirs.sort_by_key(|capture| capture.started);
    captures.extend(theirs);
    let mut channel = KickChannel {
      listed: listed.contains(&slug),
      slug,
      today_files: Files::default(),
      last_write: None,
    };
    for file in live_files(&root)? {
      if day_of(&file) == today
        && let Ok(metadata) = fs::metadata(&file)
      {
        channel.today_files.add(metadata.len());
        channel.last_write = channel.last_write.max(modified_ms(&metadata));
      }
    }
    report.channels.push(channel);
  }
  Ok((report, captures))
}

/// Free space on the disk holding `path`: the one with the longest mount
/// point that `path` is under.
fn free_space(path: &Path) -> Option<u64> {
  let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
  // canonicalize gives \\?\C:\... on Windows; mount points are C:\.
  let text = path.to_string_lossy();
  let path = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
  let disks = Disks::new_with_refreshed_list().ok()?;
  disks
    .iter()
    .filter(|disk| path.starts_with(disk.mount_point()))
    .max_by_key(|disk| disk.mount_point().as_os_str().len())
    .map(|disk| disk.available_space())
}

/// The last `lines` lines of the file at `path`, reading at most its last
/// 64 KB.
pub fn tail(path: &Path, lines: usize) -> io::Result<Vec<String>> {
  let mut file = fs::File::open(path)?;
  let size = file.metadata()?.len();
  let start = size.saturating_sub(64 << 10);
  file.seek(SeekFrom::Start(start))?;
  let mut bytes = Vec::new();
  file.read_to_end(&mut bytes)?;
  let text = String::from_utf8_lossy(&bytes);
  let mut all: Vec<&str> = text.lines().collect();
  // A cut first line is only part of one.
  if start > 0 && !all.is_empty() {
    all.remove(0);
  }
  let skip = all.len().saturating_sub(lines);
  Ok(all[skip..].iter().map(|line| (*line).to_owned()).collect())
}

/// A log in the logs directory: its name (`<name>.log`), and what it's
/// shown as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Log {
  pub name: String,
  pub shown: String,
}

impl Log {
  fn named(name: &str) -> Self {
    Self {
      name: name.into(),
      shown: name.into(),
    }
  }

  /// dgg channel `name`'s capture's, shown as `dgg-capture <name>`, as
  /// the supervisor names its capture.
  fn dgg(name: &str) -> Self {
    Self {
      name: dgg_capture_log(name),
      shown: format!("dgg-capture {name}"),
    }
  }

  /// Kick channel `slug`'s capture's, shown as `kick-capture <slug>`, as
  /// the programs box names its capture.
  fn kick(slug: &str) -> Self {
    Self {
      name: kick_capture_log(slug),
      shown: format!("kick-capture {slug}"),
    }
  }
}

/// Every log: the [`LOGS`], then each channel's capture's: destiny.gg's,
/// then each Kick channel's.
pub fn logs(report: &Report) -> Vec<Log> {
  let mut logs: Vec<Log> = LOGS.iter().map(|name| Log::named(name)).collect();
  logs.push(Log::dgg(DESTINY));
  let channels = report.kick.channels.iter();
  logs.extend(channels.map(|channel| Log::kick(&channel.slug)));
  logs
}

/// The captures' logs: destiny.gg's, then the one dgg's wrote before each
/// channel had its own, each Kick channel's, then the one Kick's shared
/// before.
pub fn capture_logs(report: &Report) -> Vec<Log> {
  let mut logs = vec![Log::dgg(DESTINY), Log::named("dgg-capture")];
  let channels = report.kick.channels.iter();
  logs.extend(channels.map(|channel| Log::kick(&channel.slug)));
  logs.push(Log::named("kick-capture"));
  logs
}

/// The last `lines` lines of `these` logs in `logs`, merged in time order,
/// each after its time, its level and what its log is shown as. A line
/// without a time of its own keeps its place after the line before it.
pub fn merge_logs(logs: &Path, these: &[Log], lines: usize) -> Vec<String> {
  let width = these.iter().map(|log| log.shown.chars().count()).max();
  let width = width.unwrap_or(0).max(12);
  // Time, log, line in the log, and the line as shown.
  let mut all: Vec<(String, usize, usize, String)> = Vec::new();
  for (log, Log { name, shown }) in these.iter().enumerate() {
    let Ok(tail) = tail(&logs.join(format!("{name}.log")), lines) else {
      continue;
    };
    let mut time = String::new();
    for (at, line) in tail.into_iter().enumerate() {
      let (own, rest) = match logged_at(&line) {
        Some(own) => {
          time = own.to_owned();
          (own, line[own.len()..].trim_start())
        }
        None => ("", line.as_str()),
      };
      let shown = match Level::split(rest) {
        Some((level, rest)) => {
          format!("{own:24} {level} {shown:<width$} {rest}")
        }
        None => format!("{own:24} {:5} {shown:<width$} {rest}", ""),
      };
      all.push((time.clone(), log, at, shown));
    }
  }
  all.sort();
  let skip = all.len().saturating_sub(lines);
  all
    .into_iter()
    .skip(skip)
    .map(|(.., shown)| shown)
    .collect()
}

/// The time a log line starts with, as [`utc_timestamp`] writes it
/// (`YYYY-MM-DD HH:MM:SS.mmmZ`), if it starts with one.
pub fn logged_at(line: &str) -> Option<&str> {
  let time = line.get(..24)?;
  let shape = time.bytes().enumerate().all(|(at, byte)| match at {
    4 | 7 => byte == b'-',
    10 => byte == b' ',
    13 | 16 => byte == b':',
    19 => byte == b'.',
    23 => byte == b'Z',
    _ => byte.is_ascii_digit(),
  });
  shape.then_some(time)
}

/// The last `lines` records capture `which` wrote to its newest live file,
/// in the journal directory `journal`, one line each: receive time
/// (`HH:MM:SS.mmmZ`, UTC), connection, and the frame as received (or what
/// happened to the connection). Reads at most the last 64 KB.
pub fn capture_tail(
  journal: &Path,
  which: &Which,
  lines: usize,
) -> io::Result<Vec<String>> {
  let files = match &which.kick {
    Some(slug) => live_files(&channel_root(&kick_root(journal), slug))?,
    None => dgg_roots(journal)?[0].live_files()?,
  };
  let Some(file) = files.into_iter().rfind(|file| {
    live_file_instance(file).is_some_and(|(theirs, _)| theirs == which.instance)
  }) else {
    return Ok(Vec::new());
  };
  let start = fs::metadata(&file)?.len().saturating_sub(64 << 10);
  let reader = Reader::open(&file, start).map_err(|e| match e {
    ::logger_journal::ReadError::Io(e) => e,
    other => io::Error::other(format!("{}: {other:?}", file.display())),
  })?;
  // Starting mid-file, the first line may be cut: it doesn't decode.
  let records: Vec<Record> = reader
    .filter_map(Result::ok)
    .map(|entry| entry.record)
    .collect();
  let skip = records.len().saturating_sub(lines);
  Ok(records[skip..].iter().map(record_line).collect())
}

/// One record, as [`capture_tail`] shows it.
fn record_line(record: &Record) -> String {
  let (receive_time, connection, what) = match record {
    Record::V0(record) => (
      record.receive_time,
      record.connection,
      match &record.body {
        v0::Body::Frame(frame) => frame.clone(),
        v0::Body::Binary(bytes) => binary(bytes),
        v0::Body::Open(url) => format!("(connected to {url})"),
        v0::Body::Close(why) => format!("(disconnected: {why})"),
        v0::Body::Fetched { source, nick, .. } => {
          format!("(fetched from {source}: {nick})")
        }
        v0::Body::FetchRange { source, .. } => {
          format!("(fetched from {source})")
        }
      },
    ),
    Record::Kick0(record) => (
      record.receive_time,
      record.connection,
      match &record.body {
        kick::v0::Body::Frame(frame) => frame.clone(),
        kick::v0::Body::Binary(bytes) => binary(bytes),
        kick::v0::Body::Open(url) => format!("(connected to {url})"),
        kick::v0::Body::Close(why) => format!("(disconnected: {why})"),
        kick::v0::Body::History { cursor, page } => format!(
          "(history page{}, {} bytes)",
          cursor
            .as_ref()
            .map(|cursor| format!(" before {cursor}"))
            .unwrap_or_default(),
          page.len()
        ),
      },
    ),
  };
  let time = utc_timestamp(receive_time);
  // Just the time of day: `HH:MM:SS.mmmZ`.
  let time = time.get(11..).unwrap_or(&time);
  format!("{time} #{connection} {what}")
}

fn binary(bytes: &[u8]) -> String {
  format!("(binary frame, {} bytes)", bytes.len())
}

/// The gaps in what the index at `path` holds, as `dgg-gaps` reports them: its
/// summary, and two lines a gap; `None` if nothing is captured yet. Unlike
/// `dgg-gaps`, it doesn't bring the index up to date first (`index --follow`
/// does), so it needs no write access.
pub fn gaps(path: &Path) -> Result<Option<(String, Vec<String>)>, String> {
  if !path.exists() {
    return Err("no index yet".into());
  }
  let db = index::open_read_only(path, Duration::from_secs(2))
    .map_err(|e| e.to_string())?;
  let coverage =
    ::logger::dgg::gaps::coverage(&db).map_err(|e| e.to_string())?;
  Ok(coverage.map(|coverage| (coverage.summary(), coverage.lines(0))))
}

/// How one capture is doing, as text.
pub fn health(capture: &Capture, now: i64) -> String {
  match capture.health {
    Health::Healthy(since) => {
      format!("healthy for {}", format::duration(now - since))
    }
    Health::Connecting => "connecting".into(),
    Health::Stale(heard) => {
      format!("STALE: last heard {} ago", format::duration(now - heard))
    }
  }
}

/// The report as lines of text (the logs not included).
pub fn text(report: &Report) -> Vec<String> {
  let now = report.now;
  let mut out = Vec::new();
  let free = report
    .free
    .map(|free| format!("   {} free", format::size(free as f64)))
    .unwrap_or_default();
  out.push(format!("data {}{free}", report.paths.data.display()));
  out.push(String::new());

  out.push("PROGRAMS".into());
  if report.programs.is_empty() {
    out.push("  none running".into());
  }
  let name = program_width(report);
  for program in &report.programs {
    let up = format::duration(now - program.started as i64 * 1000);
    let from = match (&program.exe, program.ours) {
      (_, true) => program
        .exe
        .as_ref()
        .and_then(|exe| exe.parent()?.file_name())
        .map(|build| build.to_string_lossy().into_owned())
        .unwrap_or_default(),
      (Some(exe), false) => exe.display().to_string(),
      (None, false) => "(path hidden)".into(),
    };
    out.push(format!(
      "  {:<name$}  pid {:<6} up {:<8} {:>8}  {:>5.1}% cpu  {from}",
      program.name,
      program.pid,
      up,
      format::size(program.memory as f64),
      program.cpu
    ));
  }
  out.push(String::new());

  out.push("CAPTURES".into());
  if report.captures.is_empty() {
    out.push("  none".into());
  }
  let width = listening_width(report);
  for capture in &report.captures {
    out.push(format!(
      "  {:<4} {:<30} {:<width$}  up {:<8} {:<24} {:>10} frames {:>9}  {}",
      service(capture),
      capture.instance,
      listening(capture),
      format::duration(now - capture.started),
      health(capture, now),
      format::count(capture.frames),
      per_minute(capture),
      capture.build.as_deref().unwrap_or("")
    ));
  }
  out.push(String::new());

  out.push("JOURNAL".into());
  out.extend(
    journal_lines(report)
      .into_iter()
      .map(|line| format!("  {line}")),
  );
  out
}

/// How wide the programs' column of names is: the widest name's width.
pub fn program_width(report: &Report) -> usize {
  let widest = report.programs.iter().map(|p| p.name.chars().count()).max();
  widest.unwrap_or(0)
}

/// The service `capture` is of: `dgg` or `kick`.
pub fn service(capture: &Capture) -> &'static str {
  if capture.kick { "kick" } else { "dgg" }
}

/// `capture`'s frames a minute, as shown (`12/min`); blank until known.
pub fn per_minute(capture: &Capture) -> String {
  capture
    .per_minute
    .map(|rate| format!("{}/min", format::count(rate.round() as u64)))
    .unwrap_or_default()
}

/// What `capture` listens to, as shown: `?` for dgg's before it's known.
pub fn listening(capture: &Capture) -> &str {
  capture.listening.as_deref().unwrap_or("?")
}

/// How wide the captures' column of what they listen to is.
pub fn listening_width(report: &Report) -> usize {
  let widest = report.captures.iter().map(|c| listening(c).len()).max();
  widest.unwrap_or(0)
}

/// Kick's channels' journals today, one line each, and the channel list's
/// problem, if it has one.
fn kick_lines(report: &Report) -> Vec<String> {
  let kick = &report.kick;
  let now = report.now;
  let mut out = Vec::new();
  for channel in &kick.channels {
    let last = match channel.last_write {
      Some(at) => format!("last write {} ago", format::duration(now - at)),
      None => "nothing today".into(),
    };
    let listed = if channel.listed { "" } else { "  (not listed)" };
    out.push(format!(
      "kick {:<20} today {} files  {}  {last}{listed}",
      channel.slug,
      channel.today_files.count,
      format::size(channel.today_files.bytes as f64)
    ));
  }
  if let Some(problem) = &kick.problem {
    out.push(format!("kick channel list  {problem}"));
  }
  out
}

/// The size and last write (unix ms) of the file at `path`, if there's
/// one.
fn size_and_write(path: &Path) -> Option<(u64, i64)> {
  let metadata = fs::metadata(path).ok()?;
  Some((metadata.len(), modified_ms(&metadata)?))
}

/// The journal and indexes, one line each; Kick's index only if there's
/// one.
pub fn journal_lines(report: &Report) -> Vec<String> {
  let journal = &report.journal;
  let now = report.now;
  let mut out = Vec::new();
  let last = match journal.last_write {
    Some(at) => format!("last write {} ago", format::duration(now - at)),
    None => "nothing yet".into(),
  };
  let growth = journal
    .growth
    .map(|rate| format!("  +{}/s", format::size(rate)))
    .unwrap_or_default();
  out.push(format!(
    "today (UTC {})  {} files  {}  {last}{growth}",
    journal.today,
    journal.today_files.count,
    format::size(journal.today_files.bytes as f64)
  ));
  if let Some(first) = journal.waiting.first() {
    out.push(format!(
      "waiting for compact  {} days (UTC) from {first}  {}",
      journal.waiting.len(),
      format::size(journal.waiting_files.bytes as f64)
    ));
  }
  let archived = |label: &str, archived: &Archived| {
    archived.first.as_ref().map(|first| {
      format!(
        "{label}  {} days (UTC), {first} to {}  {}",
        archived.days,
        archived.last.as_deref().unwrap_or(first),
        format::size(archived.bytes as f64)
      )
    })
  };
  out.extend(archived("archived", &journal.archived));
  if journal.backfill_waiting.count > 0 {
    out.push(format!(
      "backfill waiting for compact  {} files  {}",
      journal.backfill_waiting.count,
      format::size(journal.backfill_waiting.bytes as f64)
    ));
  }
  out.extend(archived("backfill archived", &journal.backfill_archived));
  out.extend(kick_lines(report));
  let index = |label: &str, (bytes, at): (u64, i64)| {
    format!(
      "{label}  {}  last write {} ago",
      format::size(bytes as f64),
      format::duration(now - at)
    )
  };
  out.push(match report.index {
    Some(index_file) => index("index", index_file),
    None => "index  none yet".into(),
  });
  out.extend(report.kick_index.map(|file| index("kick index", file)));
  if let Some(sync) = &report.sync {
    for peer in &sync.peers {
      let last = peer.last.as_deref().unwrap_or("no round yet");
      out.push(format!(
        "sync from {} ({})  last: {last}",
        peer.name, peer.url
      ));
    }
    for problem in &sync.problems {
      out.push(format!("sync  {problem}"));
    }
  }
  out
}
