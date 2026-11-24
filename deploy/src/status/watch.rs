//! `status --watch`: the report redrawn every second, with a page filling
//! the rest of the screen.
//!
//! The keys are grouped. First, with letters: `l` every log at once, in
//! time order, the page it starts on; `r` the newest records a capture
//! wrote, again for the next capture; Space redraws now, `e` opens the data
//! folder, and `q` (or Esc, or Ctrl-C) quits. Then each program's log, with
//! numbers: `1` the supervisor's, `2` migrate's, `3` capture's, `4`
//! compact's, `5` index's, `6` the gaps in capture, looked at again every
//! 30 seconds, and `7` sync's. Last, `8` kick-capture's log and `9`
//! serve's. On a narrow screen the keys take more than one line.

use {
  super::{
    Report, Watcher, all_logs, capture_tail, gaps, health, journal_lines,
    kick_lines, logged_at, tail,
  },
  crate::paths::Paths,
  ::common::{
    format,
    log::Level,
    time::{now_ms, utc_timestamp},
  },
  ::logger::{dgg::Dgg, service::Service},
  ::ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph},
  },
  ::std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
  },
};

/// What a key does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
  Show(Page),
  Redraw,
  OpenData,
  Quit,
}

/// A page for the bottom of the screen, as a key offers it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
  /// Every log at once.
  Logs,
  /// One of [`LOGS`](super::LOGS).
  Log(&'static str),
  Records,
  Gaps,
}

/// The keys, with their names and what they do, in groups: the views
/// across everything and the controls, then each program's log, then
/// Kick's and serve's.
const KEYS: [&[(char, &str, Action)]; 3] = [
  &[
    ('l', "logs", Action::Show(Page::Logs)),
    ('r', "records", Action::Show(Page::Records)),
    (' ', "redraw", Action::Redraw),
    ('e', "open data", Action::OpenData),
    ('q', "quit", Action::Quit),
  ],
  &[
    ('1', "supervisor", Action::Show(Page::Log("supervisor"))),
    ('2', "migrate", Action::Show(Page::Log("migrate"))),
    ('3', "dgg-capture", Action::Show(Page::Log("dgg-capture"))),
    ('4', "compact", Action::Show(Page::Log("compact"))),
    ('5', "index", Action::Show(Page::Log("index"))),
    ('6', "gaps", Action::Show(Page::Gaps)),
    ('7', "sync", Action::Show(Page::Log("sync"))),
  ],
  &[
    ('8', "kick", Action::Show(Page::Log("kick-capture"))),
    ('9', "serve", Action::Show(Page::Log("serve"))),
  ],
];

/// How often the gaps are looked at again while they're shown: looking
/// takes a moment, so it isn't redone every redraw.
const GAPS_EVERY: Duration = Duration::from_secs(30);

/// What fills the bottom of the screen.
enum View {
  /// Every log at once.
  Logs,
  /// One of [`LOGS`](super::LOGS).
  Log(&'static str),
  /// The newest records of a capture instance, if one is running.
  Records(Option<String>),
  Gaps(Gaps),
}

/// The gaps page: what was found last, and the next look.
struct Gaps {
  /// What was found last, if anything yet.
  pane: Option<Pane<'static>>,
  /// The look under way, on a thread of its own.
  looking: Option<Receiver<Pane<'static>>>,
  /// When to look again.
  next: Instant,
}

/// The bottom of the screen: a title, a line kept at the top, its lines
/// (the last of them shown, as many as fit), and what to say if there are
/// none.
#[derive(Clone)]
pub struct Pane<'a> {
  pub title: String,
  pub header: Option<String>,
  pub lines: Vec<String>,
  pub empty: &'a str,
}

impl Gaps {
  fn new() -> Self {
    Self {
      pane: None,
      looking: None,
      next: Instant::now(),
    }
  }

  /// Takes what the look under way found, if it's done, and starts the next
  /// look at the index at `index` when it's due.
  fn refresh(&mut self, index: PathBuf) {
    if let Some(looking) = &self.looking
      && let Ok(pane) = looking.try_recv()
    {
      self.pane = Some(pane);
      self.looking = None;
      self.next = Instant::now() + GAPS_EVERY;
    }
    if self.looking.is_none() && Instant::now() >= self.next {
      let (found, looking) = mpsc::channel();
      thread::spawn(move || found.send(look_for_gaps(&index)));
      self.looking = Some(looking);
    }
  }
}

/// The gaps, as the index at `index` has them now.
fn look_for_gaps(index: &Path) -> Pane<'static> {
  let now = now_ms();
  let (header, lines, empty) = match gaps(index) {
    Ok(Some((summary, lines))) => (Some(summary), lines, "(no gaps)"),
    Ok(None) => (None, Vec::new(), "(nothing captured yet)"),
    Err(e) => (None, vec![format!("(can't read the index: {e})")], ""),
  };
  Pane {
    title: format!(
      "gaps as of {} (again every {}s)",
      utc_timestamp(now),
      GAPS_EVERY.as_secs()
    ),
    header,
    lines,
    empty,
  }
}

impl View {
  /// What shows `page`, its key pressed while this shows: records' key
  /// again shows the next capture's records.
  fn show(self, page: Page, report: &Report) -> Self {
    match (page, self) {
      (Page::Logs, _) => Self::Logs,
      (Page::Log(name), _) => Self::Log(name),
      (Page::Records, current) => current.next_records(report),
      (Page::Gaps, gaps @ Self::Gaps(_)) => gaps,
      (Page::Gaps, _) => Self::Gaps(Gaps::new()),
    }
  }

  fn pane(&self, paths: &Paths) -> Pane<'static> {
    match self {
      Self::Logs => Pane {
        title: "every log, in time order".into(),
        header: None,
        lines: all_logs(&paths.logs, 500),
        empty: "(empty: shell runs print to their own windows)",
      },
      Self::Log(log) => Pane {
        title: format!("{log}.log"),
        header: None,
        lines: tail(&paths.logs.join(format!("{log}.log")), 500)
          .unwrap_or_default(),
        empty: "(empty: shell runs print to their own windows)",
      },
      Self::Records(Some(instance)) => Pane {
        title: format!("records: {instance} (r again for the next capture)"),
        header: None,
        lines: capture_tail(&paths.data.join("journal"), instance, 500)
          .unwrap_or_else(|e| vec![format!("(can't read: {e})")]),
        empty: "(no live file yet)",
      },
      Self::Records(None) => Pane {
        title: "records".into(),
        header: None,
        lines: Vec::new(),
        empty: "(no capture running)",
      },
      Self::Gaps(Gaps {
        pane: Some(pane), ..
      }) => pane.clone(),
      Self::Gaps(Gaps { pane: None, .. }) => Pane {
        title: "gaps".into(),
        header: None,
        lines: Vec::new(),
        empty: "(looking...)",
      },
    }
  }

  /// The newest capture's records, or if a capture's are shown, the next
  /// older one's.
  fn next_records(&self, report: &Report) -> Self {
    let instances: Vec<&str> = report
      .captures
      .iter()
      .rev()
      .map(|capture| capture.instance.as_str())
      .collect();
    let next = match self {
      Self::Records(Some(current)) => instances
        .iter()
        .position(|instance| instance == current)
        .map_or(0, |at| (at + 1) % instances.len()),
      Self::Records(None) | Self::Logs | Self::Log(_) | Self::Gaps(_) => 0,
    };
    Self::Records(instances.get(next).map(|instance| (*instance).to_owned()))
  }
}

/// Runs the screen until the user quits.
pub fn run(paths: &Paths, interval: Duration) -> io::Result<()> {
  let mut watcher = Watcher::new()?;
  let mut terminal = ::ratatui::try_init()?;
  let result = event_loop(&mut terminal, &mut watcher, paths, interval);
  ::ratatui::restore();
  result
}

fn event_loop(
  terminal: &mut DefaultTerminal,
  watcher: &mut Watcher,
  paths: &Paths,
  interval: Duration,
) -> io::Result<()> {
  let mut view = View::Logs;
  loop {
    let report = watcher.gather(paths)?;
    if let View::Gaps(gaps) = &mut view {
      gaps.refresh(paths.data.join(Dgg::INDEX));
    }
    let pane = view.pane(paths);
    terminal.draw(|frame| draw(frame, &report, &pane))?;

    let until = Instant::now() + interval;
    loop {
      let left = until.saturating_duration_since(Instant::now());
      if left.is_zero() || !event::poll(left)? {
        break;
      }
      let Event::Key(key) = event::read()? else {
        // A resize, say: redraw.
        break;
      };
      if key.kind != KeyEventKind::Press {
        continue;
      }
      match key.code {
        KeyCode::Esc => return Ok(()),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
          return Ok(());
        }
        KeyCode::Char(key) => match action(key) {
          Some(Action::Show(page)) => {
            view = view.show(page, &report);
            break;
          }
          Some(Action::Redraw) => break,
          Some(Action::OpenData) => open(&paths.data),
          Some(Action::Quit) => return Ok(()),
          None => {}
        },
        _ => {}
      }
    }
  }
}

/// What `key` does, if anything.
fn action(key: char) -> Option<Action> {
  KEYS
    .iter()
    .flat_map(|group| group.iter())
    .find(|(offered, _, _)| *offered == key)
    .map(|(_, _, action)| *action)
}

/// The keys, in their groups, on as many lines `width` wide as they need. A
/// group isn't split across lines, and goes on the first line with room.
pub fn keys(width: u16) -> Vec<String> {
  let groups: Vec<String> = KEYS
    .iter()
    .map(|group| {
      group
        .iter()
        .map(|(key, name, _)| match key {
          ' ' => format!("space {name}"),
          key => format!("{key} {name}"),
        })
        .collect::<Vec<_>>()
        .join("  ")
    })
    .collect();
  let fits =
    |line: &String, group: &str| line.len() + 3 + group.len() <= width as usize;
  let mut lines: Vec<String> = Vec::new();
  for group in groups {
    match lines.iter_mut().find(|line| fits(line, &group)) {
      Some(line) => {
        line.push_str(" | ");
        line.push_str(&group);
      }
      None => lines.push(format!(" {group}")),
    }
  }
  lines
}

/// Opens `path` in the system's file manager, if it can.
fn open(path: &Path) {
  let program = if cfg!(windows) {
    "explorer"
  } else if cfg!(target_os = "macos") {
    "open"
  } else {
    "xdg-open"
  };
  let _ = Command::new(program).arg(path).spawn();
}

/// `line` drawn as a log's: its time dimmed, and its level in colour, the
/// message too if it's a warning or an error. Lines that aren't a log's are
/// drawn as they are.
fn log_line(line: &str) -> Line<'_> {
  let Some(time) = logged_at(line) else {
    return Line::from(line);
  };
  let rest = &line[time.len()..];
  let Some((level, message)) = Level::split(rest.trim_start()) else {
    return Line::from(vec![Span::raw(time).dark_gray(), Span::raw(rest)]);
  };
  let colour = match level {
    Level::Info => Style::new(),
    Level::Warn => Style::new().fg(Color::Yellow),
    Level::Error => Style::new().fg(Color::Red),
  };
  let name = match level {
    Level::Info => Style::new().fg(Color::Green),
    _ => colour.bold(),
  };
  Line::from(vec![
    Span::raw(time).dark_gray(),
    Span::raw(" "),
    Span::styled(level.to_string(), name),
    Span::raw(" "),
    Span::styled(message, colour),
  ])
}

fn heading(title: &str) -> Block<'_> {
  Block::bordered().title(Span::styled(
    format!(" {title} "),
    Style::new().add_modifier(Modifier::BOLD),
  ))
}

/// Draws `report`, with `pane` below.
pub fn draw(frame: &mut Frame, report: &Report, pane: &Pane) {
  let now = report.now;
  let programs: Vec<Line> = if report.programs.is_empty() {
    vec![Line::from("none running".dark_gray())]
  } else {
    report
      .programs
      .iter()
      .map(|program| {
        let up = format::duration(now - program.started as i64 * 1000);
        let from = match (&program.exe, program.ours) {
          (Some(exe), true) => exe
            .parent()
            .and_then(|build| build.file_name())
            .map(|build| build.to_string_lossy().into_owned())
            .unwrap_or_default(),
          (Some(exe), false) => exe.display().to_string(),
          (None, _) => "(path hidden)".into(),
        };
        Line::from(vec![
          Span::styled(format!("{:<18}", program.name), Style::new().bold()),
          Span::raw(format!(
            " pid {:<6} up {:<8} {:>8} {:>6.1}% cpu  ",
            program.pid,
            up,
            format::size(program.memory as f64),
            program.cpu
          )),
          Span::styled(from, Style::new().dark_gray()),
        ])
      })
      .collect()
  };
  let captures: Vec<Line> = if report.captures.is_empty() {
    vec![Line::from("none".dark_gray())]
  } else {
    report
      .captures
      .iter()
      .map(|capture| {
        let color = match capture.health {
          super::Health::Healthy(_) => Color::Green,
          super::Health::Connecting => Color::Yellow,
          super::Health::Stale(_) => Color::Red,
        };
        Line::from(vec![
          Span::raw(format!(
            "{:<30} up {:<8} ",
            capture.instance,
            format::duration(now - capture.started)
          )),
          Span::styled(
            format!("{:<24} ", health(capture, now)),
            Style::new().fg(color),
          ),
          Span::raw(format!("{:>10} frames  ", format::count(capture.frames))),
          Span::styled(
            capture.build.clone().unwrap_or_default(),
            Style::new().dark_gray(),
          ),
        ])
      })
      .collect()
  };
  let journal: Vec<Line> =
    journal_lines(report).into_iter().map(Line::from).collect();
  let kick: Vec<Line> =
    kick_lines(report).into_iter().map(Line::from).collect();

  let free = report
    .free
    .map(|free| format!("   {} free", format::size(free as f64)))
    .unwrap_or_default();
  let header = Line::from(vec![
    Span::styled(" dgg-logger ", Style::new().bold().reversed()),
    Span::raw(format!(
      "  {}  data {}{free}",
      utc_timestamp(now),
      report.paths.data.display()
    )),
  ]);

  let keys: Vec<Line> = keys(frame.area().width)
    .into_iter()
    .map(|line| Line::from(line.dark_gray()))
    .collect();
  // Kick's block only when Kick is captured.
  let kick_height = match kick.len() {
    0 => 0,
    lines => lines as u16 + 2,
  };
  let [
    top,
    programs_area,
    captures_area,
    journal_area,
    kick_area,
    log_area,
    keys_area,
  ] = Layout::vertical([
    Constraint::Length(1),
    Constraint::Length(programs.len() as u16 + 2),
    Constraint::Length(captures.len() as u16 + 2),
    Constraint::Length(journal.len() as u16 + 2),
    Constraint::Length(kick_height),
    Constraint::Min(3),
    Constraint::Length(keys.len() as u16),
  ])
  .areas(frame.area());
  frame.render_widget(header, top);
  frame.render_widget(
    Paragraph::new(programs).block(heading("programs")),
    programs_area,
  );
  frame.render_widget(
    Paragraph::new(captures).block(heading("captures")),
    captures_area,
  );
  frame.render_widget(
    Paragraph::new(journal).block(heading("journal")),
    journal_area,
  );
  if !kick.is_empty() {
    frame.render_widget(Paragraph::new(kick).block(heading("kick")), kick_area);
  }
  let mut body: Vec<Line> = Vec::new();
  if let Some(header) = &pane.header {
    body.push(Line::from(header.as_str()).bold());
  }
  let room =
    (log_area.height.saturating_sub(2) as usize).saturating_sub(body.len());
  let shown = &pane.lines[pane.lines.len().saturating_sub(room)..];
  if shown.is_empty() {
    body.push(Line::from(pane.empty.dark_gray()));
  } else {
    body.extend(shown.iter().map(|line| log_line(line)));
  }
  frame
    .render_widget(Paragraph::new(body).block(heading(&pane.title)), log_area);
  frame.render_widget(Paragraph::new(keys), keys_area);
}
