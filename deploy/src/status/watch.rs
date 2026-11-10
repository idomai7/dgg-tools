//! `status --watch`: the report redrawn every few seconds, with a log
//! filling the rest of the screen.
//!
//! Keys: `1`-`5` show the capture, index, compact, supervisor or sync
//! log, `f` the newest frames a capture wrote (again for the next
//! capture), `g` the gaps in capture (again to look again), `r` redraws
//! now, `e` opens the data folder, `q` (or Esc, or Ctrl-C) quits.

use {
  super::{
    LOGS, Report, Watcher, capture_tail, format, gaps, health, journal_lines,
    tail,
  },
  crate::{
    log::{now_ms, timestamp},
    paths::Paths,
  },
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
    process::Command,
    time::{Duration, Instant},
  },
};

const KEYS: &str = " 1 capture  2 index  3 compact  4 supervisor  5 sync  f frames  g gaps   r redraw  e open data  q quit";

/// What fills the bottom of the screen.
enum View {
  /// One of [`LOGS`].
  Log(usize),
  /// The newest frames of a capture instance.
  Frames(String),
  /// The gaps, as of when `g` was pressed: looking takes a moment, so it
  /// isn't redone every redraw.
  Gaps(Pane<'static>),
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

impl View {
  /// `g` pressed: the gaps, as the index has them now.
  fn gaps(paths: &Paths) -> Self {
    let now = now_ms();
    let (header, lines, empty) = match gaps(&paths.data.join("index.sqlite")) {
      Ok(Some((summary, lines))) => (Some(summary), lines, "(no gaps)"),
      Ok(None) => (None, Vec::new(), "(nothing captured yet)"),
      Err(e) => (None, vec![format!("(can't read the index: {e})")], ""),
    };
    let pane = Pane {
      title: format!("gaps as of {} (g to look again)", timestamp(now)),
      header,
      lines,
      empty,
    };
    Self::Gaps(pane)
  }
}

impl View {
  fn pane(&self, paths: &Paths) -> Pane<'static> {
    match self {
      Self::Log(log) => Pane {
        title: format!("{}.log", LOGS[*log]),
        header: None,
        lines: tail(&paths.logs.join(format!("{}.log", LOGS[*log])), 500)
          .unwrap_or_default(),
        empty: "(empty: shell runs print to their own windows)",
      },
      Self::Frames(instance) => Pane {
        title: format!("frames: {instance}"),
        header: None,
        lines: capture_tail(&paths.data.join("journal"), instance, 500)
          .unwrap_or_else(|e| vec![format!("(can't read: {e})")]),
        empty: "(no live file yet)",
      },
      Self::Gaps(pane) => pane.clone(),
    }
  }

  /// `f` pressed: the newest capture's frames, or the next older one's.
  fn next_frames(&self, report: &Report) -> Self {
    let instances: Vec<&str> = report
      .captures
      .iter()
      .rev()
      .map(|capture| capture.instance.as_str())
      .collect();
    let next = match self {
      Self::Frames(current) => instances
        .iter()
        .position(|instance| instance == current)
        .map_or(0, |at| (at + 1) % instances.len()),
      Self::Log(_) | Self::Gaps(_) => 0,
    };
    match instances.get(next) {
      Some(instance) => Self::Frames((*instance).to_owned()),
      None => Self::Log(0),
    }
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
  let mut view = View::Log(0);
  loop {
    let report = watcher.gather(paths)?;
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
        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
          return Ok(());
        }
        KeyCode::Char(digit @ '1'..='5') => {
          view = View::Log(digit as usize - '1' as usize);
          break;
        }
        KeyCode::Char('f') => {
          view = view.next_frames(&report);
          break;
        }
        KeyCode::Char('g') => {
          view = View::gaps(paths);
          break;
        }
        KeyCode::Char('r') => break,
        KeyCode::Char('e') => open(&paths.data),
        _ => {}
      }
    }
  }
}

/// Opens `path` in the system's file manager, if it can.
fn open(path: &::std::path::Path) {
  let program = if cfg!(windows) {
    "explorer"
  } else if cfg!(target_os = "macos") {
    "open"
  } else {
    "xdg-open"
  };
  let _ = Command::new(program).arg(path).spawn();
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
        let up = format::age(now - program.started as i64 * 1000);
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
            format::age(now - capture.started)
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

  let free = report
    .free
    .map(|free| format!("   {} free", format::size(free as f64)))
    .unwrap_or_default();
  let header = Line::from(vec![
    Span::styled(" dgg-logger ", Style::new().bold().reversed()),
    Span::raw(format!(
      "  {}  data {}{free}",
      crate::log::timestamp(now),
      report.paths.data.display()
    )),
  ]);

  let [
    top,
    programs_area,
    captures_area,
    journal_area,
    log_area,
    keys,
  ] = Layout::vertical([
    Constraint::Length(1),
    Constraint::Length(programs.len() as u16 + 2),
    Constraint::Length(captures.len() as u16 + 2),
    Constraint::Length(journal.len() as u16 + 2),
    Constraint::Min(3),
    Constraint::Length(1),
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
    body.extend(shown.iter().map(|line| Line::from(line.as_str())));
  }
  frame
    .render_widget(Paragraph::new(body).block(heading(&pane.title)), log_area);
  frame.render_widget(Line::from(KEYS.dark_gray()), keys);
}
