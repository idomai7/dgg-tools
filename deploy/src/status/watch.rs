//! `status --watch`: the report redrawn every few seconds, with a log
//! filling the rest of the screen.
//!
//! Keys: `1`-`4` show the capture, index, compact or supervisor log, `r`
//! redraws now, `e` opens the data folder, `q` (or Esc, or Ctrl-C) quits.

use {
  super::{LOGS, Report, Watcher, format, health, journal_lines, tail},
  crate::paths::Paths,
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

const KEYS: &str = " 1 capture  2 index  3 compact  4 supervisor   r redraw  e open data  q quit";

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
  let mut log = 0;
  loop {
    let report = watcher.gather(paths)?;
    let lines = tail(&paths.logs.join(format!("{}.log", LOGS[log])), 500)
      .unwrap_or_default();
    terminal.draw(|frame| draw(frame, &report, LOGS[log], &lines))?;

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
        KeyCode::Char(digit @ '1'..='4') => {
          log = digit as usize - '1' as usize;
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

/// Draws `report`, with `log_lines` of the `log` log below.
pub fn draw(
  frame: &mut Frame,
  report: &Report,
  log: &str,
  log_lines: &[String],
) {
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
          Span::styled(health(capture, now), Style::new().fg(color)),
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
  let room = log_area.height.saturating_sub(2) as usize;
  let shown = &log_lines[log_lines.len().saturating_sub(room)..];
  let body: Vec<Line> = if shown.is_empty() {
    vec![Line::from(
      "(empty: shell runs print to their own windows)".dark_gray(),
    )]
  } else {
    shown.iter().map(|line| Line::from(line.as_str())).collect()
  };
  frame.render_widget(
    Paragraph::new(body).block(heading(&format!("{log}.log"))),
    log_area,
  );
  frame.render_widget(Line::from(KEYS.dark_gray()), keys);
}
