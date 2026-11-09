//! What the logger is doing: its programs, each capture's health, the
//! journal and index, free disk space and the ends of the logs.
//!
//! [`Watcher::gather`] takes a snapshot ([`Report`]); [`text`] prints one,
//! and [`watch`] redraws one every few seconds.

pub mod format;
pub mod watch;

use {
  crate::{log::now_ms, paths::Paths, processes},
  ::journal::{
    archive,
    layout::{backfill_files, backfill_root, live_files, utc_day},
  },
  ::logger::control,
  ::std::{
    collections::BTreeSet,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
  },
  ::sysinfo::{
    Disks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind,
  },
};

/// A capture status counts as stale after this long without a heartbeat,
/// as capture itself judges it.
pub const STALE_MS: i64 = 5000;

/// The logs a supervisor writes, in the order they're offered.
pub const LOGS: [&str; 4] = ["capture", "index", "compact", "supervisor"];

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
  pub instance: String,
  /// Unix ms.
  pub started: i64,
  pub health: Health,
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
  pub captures: Vec<Capture>,
  pub journal: Journal,
  /// The index's size and last write (unix ms).
  pub index: Option<(u64, i64)>,
  /// Free space on the data directory's disk.
  pub free: Option<u64>,
}

/// Takes snapshots, remembering what it needs from the last one (CPU use,
/// journal growth).
pub struct Watcher {
  system: System,
  previous: Option<(String, i64, u64)>,
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
    })
  }

  pub fn gather(&mut self, paths: &Paths) -> io::Result<Report> {
    let now = now_ms();
    let journal_root = paths.data.join("journal");
    Ok(Report {
      now,
      paths: paths.clone(),
      programs: self.programs(&paths.bin()),
      captures: captures(&journal_root, now)?,
      journal: self.journal(&journal_root, now)?,
      index: fs::metadata(paths.data.join("index.sqlite"))
        .ok()
        .and_then(|metadata| Some((metadata.len(), modified_ms(&metadata)?))),
      free: free_space(&paths.data),
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
        let name = processes::program(process);
        // Its own name, or (before the rename) anything run from the bin.
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

  fn journal(&mut self, root: &Path, now: i64) -> io::Result<Journal> {
    let today = utc_day(now);
    let mut journal = Journal {
      today: today.clone(),
      ..Journal::default()
    };
    let mut waiting = BTreeSet::new();
    for file in live_files(root)? {
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
    for file in backfill_files(root)? {
      if let Ok(metadata) = fs::metadata(&file) {
        journal.backfill_waiting.add(metadata.len());
      }
    }
    journal.archived = archived(&archive::all(root)?);
    journal.backfill_archived = archived(&archive::all(&backfill_root(root))?);

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

fn captures(journal_root: &Path, now: i64) -> io::Result<Vec<Capture>> {
  let mut captures: Vec<Capture> =
    control::read_all(&control::dir(journal_root))?
      .into_iter()
      .map(|status| Capture {
        health: if now - status.heartbeat > STALE_MS {
          Health::Stale(status.heartbeat)
        } else if let Some(since) = status.healthy_since {
          Health::Healthy(since)
        } else {
          Health::Connecting
        },
        instance: status.instance,
        started: status.started,
      })
      .collect();
  captures.sort_by_key(|capture| capture.started);
  Ok(captures)
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

/// How one capture is doing, as text.
pub fn health(capture: &Capture, now: i64) -> String {
  match capture.health {
    Health::Healthy(since) => {
      format!("healthy for {}", format::age(now - since))
    }
    Health::Connecting => "connecting".into(),
    Health::Stale(heard) => {
      format!("STALE: last heard {} ago", format::age(now - heard))
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
  for program in &report.programs {
    let up = format::age(now - program.started as i64 * 1000);
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
      "  {:<18} pid {:<6} up {:<8} {:>8}  {:>5.1}% cpu  {from}",
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
  for capture in &report.captures {
    out.push(format!(
      "  {:<30} up {:<8} {}",
      capture.instance,
      format::age(now - capture.started),
      health(capture, now)
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

/// The journal and index, one line each.
pub fn journal_lines(report: &Report) -> Vec<String> {
  let journal = &report.journal;
  let now = report.now;
  let mut out = Vec::new();
  let last = match journal.last_write {
    Some(at) => format!("last write {} ago", format::age(now - at)),
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
      "waiting for compact  {} days from {first}  {}",
      journal.waiting.len(),
      format::size(journal.waiting_files.bytes as f64)
    ));
  }
  let archived = |label: &str, archived: &Archived| {
    archived.first.as_ref().map(|first| {
      format!(
        "{label}  {} days, {first} to {}  {}",
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
  out.push(match report.index {
    Some((bytes, at)) => format!(
      "index  {}  last write {} ago",
      format::size(bytes as f64),
      format::age(now - at)
    ),
    None => "index  none yet".into(),
  });
  out
}
