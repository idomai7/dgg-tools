//! `status --watch`: the report redrawn every second, with a page filling
//! the rest of the screen.
//!
//! The keys are grouped. First, with letters: `l` every log at once, in
//! time order, the page it starts on; `r` the newest records a capture
//! wrote, again for the next capture, round every channel's; `f` one
//! channel's capture's log, again for the next; Space redraws now, `e`
//! opens the data folder, and `q` (or Esc, or Ctrl-C) quits. Then each
//! program's log, with numbers: `1` the supervisor's, `2` migrate's, `3`
//! every capture's, dgg's and Kick's, in time order, `4` compact's, `5`
//! index's, `6` the gaps in the first dgg channel's capture, looked at
//! again every 30 seconds, again for the next channel's, `7` sync's and
//! `8` serve's. Last, the
//! boxes: Tab and Shift-Tab move the focus between them (the page at the
//! bottom too), Up, Down, Page Up and Page Down scroll the focused one when
//! it's over-tall, `m` minimizes it to its title line, or restores it,
//! and `s` sorts it by its next key, `S` the other way (see [`sort`]; the
//! page keeps its order). On a narrow screen the keys take more than one
//! line.

use {
  super::{
    Log, Report, SHARED_CAPTURE_LOGS, Watcher, Which, capture_logs,
    capture_tail, gaps, health, journal_lines_in, listening, listening_width,
    logged_at, logs, merge_logs, per_minute, program_build, program_width,
    service,
    sort::{self, Order},
    tail,
  },
  crate::paths::Paths,
  ::common::{
    format,
    log::Level,
    time::{now_ms, utc_timestamp},
  },
  ::logger::dgg::{self, channels::DESTINY},
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
    path::Path,
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
  Minimize,
  Sort,
  Reverse,
}

/// A page for the bottom of the screen, as a key offers it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
  /// Every log at once.
  Logs,
  /// One of [`LOGS`](super::LOGS).
  Log(&'static str),
  /// Every capture's log.
  Captures,
  /// One capture's log, the next.
  Capture,
  Records,
  Gaps,
}

/// The keys, with their names and what they do, in groups: the views
/// across everything and the controls, then each program's log, then the
/// boxes' (after [`MOVING`]).
const KEYS: [&[(char, &str, Action)]; 3] = [
  &[
    ('l', "logs", Action::Show(Page::Logs)),
    ('r', "records", Action::Show(Page::Records)),
    ('f', "channel", Action::Show(Page::Capture)),
    (' ', "redraw", Action::Redraw),
    ('e', "open data", Action::OpenData),
    ('q', "quit", Action::Quit),
  ],
  &[
    ('1', "supervisor", Action::Show(Page::Log("supervisor"))),
    ('2', "migrate", Action::Show(Page::Log("migrate"))),
    ('3', "capture", Action::Show(Page::Captures)),
    ('4', "compact", Action::Show(Page::Log("compact"))),
    ('5', "index", Action::Show(Page::Log("index"))),
    ('6', "gaps", Action::Show(Page::Gaps)),
    ('7', "sync", Action::Show(Page::Log("sync"))),
    ('8', "serve", Action::Show(Page::Log("serve"))),
  ],
  &[
    ('m', "minimize", Action::Minimize),
    ('s', "sort", Action::Sort),
    ('S', "reverse", Action::Reverse),
  ],
];

/// The keys that aren't characters, before the boxes' group of [`KEYS`].
const MOVING: &str = "tab focus  arrows/pgup/pgdn scroll";

/// The boxes, in the order Tab moves through them: the page last.
const PROGRAMS: usize = 0;
const CAPTURES: usize = 1;
const JOURNAL: usize = 2;
const PAGE: usize = 3;

/// The fewest rows the page is given, unless it's minimized.
const PAGE_LEAST: u16 = 6;

/// The boxes' state: which has the focus, how far each is scrolled, which
/// are minimized, and the order each above the page is sorted in.
pub struct Boxes {
  pub focus: usize,
  /// The boxes above the page, from their first line; the page, back from
  /// its last.
  pub scroll: [usize; 4],
  pub minimized: [bool; 4],
  pub order: [Order; 3],
  /// How far each could scroll, and how many lines it showed, as last
  /// drawn.
  most: [usize; 4],
  rows: [usize; 4],
}

impl Default for Boxes {
  /// The page focused, nothing scrolled or minimized.
  fn default() -> Self {
    Self {
      focus: PAGE,
      scroll: [0; 4],
      minimized: [false; 4],
      order: [Order::default(); 3],
      most: [0; 4],
      rows: [0; 4],
    }
  }
}

impl Boxes {
  fn next(&mut self) {
    self.focus = (self.focus + 1) % 4;
  }

  fn previous(&mut self) {
    self.focus = (self.focus + 3) % 4;
  }

  /// Scrolls the focused box `rows` lines (a page's worth, if none) up or
  /// down, as far as it goes.
  fn scroll(&mut self, up: bool, rows: Option<usize>) {
    let at = self.focus;
    let rows = rows.unwrap_or(self.rows[at].max(1));
    // The page counts back from its end.
    let toward_zero = up != (at == PAGE);
    let scroll = &mut self.scroll[at];
    *scroll = match toward_zero {
      true => scroll.saturating_sub(rows),
      false => (*scroll + rows).min(self.most[at]),
    };
  }

  fn minimize(&mut self) {
    self.minimized[self.focus] ^= true;
  }

  /// Sorts the focused box by its next key, or reversed; not the page.
  fn sort(&mut self, reverse: bool) {
    let keys = match self.focus {
      PROGRAMS => sort::PROGRAMS.len(),
      CAPTURES => sort::CAPTURES.len(),
      JOURNAL => sort::JOURNAL.len(),
      _ => return,
    };
    let order = &mut self.order[self.focus];
    match reverse {
      true => order.reversed ^= true,
      false => order.next(keys),
    }
  }

  /// Sorts `report`'s programs and captures as their boxes are.
  pub fn sort_report(&self, report: &mut Report) {
    sort::programs(&mut report.programs, self.order[PROGRAMS]);
    sort::captures(&mut report.captures, self.order[CAPTURES]);
  }
}

/// How often the gaps are looked at again while they're shown: looking
/// takes a moment, so it isn't redone every redraw.
const GAPS_EVERY: Duration = Duration::from_secs(30);

/// What fills the bottom of the screen.
enum View {
  /// Every log at once.
  Logs,
  /// One of [`LOGS`](super::LOGS).
  Log(&'static str),
  /// The captures' logs, or just one's, by the name it's shown as.
  Captures(Option<String>),
  /// The newest records of a capture, if one is running.
  Records(Option<Which>),
  Gaps(Gaps),
}

/// The gaps page: the dgg channel's, what was found last, and the next
/// look.
struct Gaps {
  channel: String,
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
  fn new(channel: String) -> Self {
    Self {
      channel,
      pane: None,
      looking: None,
      next: Instant::now(),
    }
  }

  /// Takes what the look under way found, if it's done, and starts the next
  /// look at the channel's index, in the data directory `data`, when it's
  /// due; `more` if there are other channels to go round to.
  fn refresh(&mut self, data: &Path, more: bool) {
    if let Some(looking) = &self.looking
      && let Ok(pane) = looking.try_recv()
    {
      self.pane = Some(pane);
      self.looking = None;
      self.next = Instant::now() + GAPS_EVERY;
    }
    if self.looking.is_none() && Instant::now() >= self.next {
      let index = data.join(dgg::index::file(&self.channel));
      let channel = self.channel.clone();
      let (found, looking) = mpsc::channel();
      thread::spawn(move || found.send(look_for_gaps(&index, &channel, more)));
      self.looking = Some(looking);
    }
  }
}

/// The gaps in dgg channel `channel`'s capture, as its index at `index`
/// has them now; `more` if there are other channels to go round to.
fn look_for_gaps(index: &Path, channel: &str, more: bool) -> Pane<'static> {
  let now = now_ms();
  let (header, lines, empty) = match gaps(index) {
    Ok(Some((summary, lines))) => (Some(summary), lines, "(no gaps)"),
    Ok(None) => (None, Vec::new(), "(nothing captured yet)"),
    Err(e) => (None, vec![format!("(can't read the index: {e})")], ""),
  };
  Pane {
    title: format!(
      "gaps in {channel} as of {} (again every {}s{})",
      utc_timestamp(now),
      GAPS_EVERY.as_secs(),
      if more {
        "; 6 again for the next channel"
      } else {
        ""
      }
    ),
    header,
    lines,
    empty,
  }
}

impl View {
  /// What shows `page`, its key pressed while this shows: records' key
  /// again shows the next capture's records, and the gaps' the next
  /// channel's.
  fn show(self, page: Page, report: &Report) -> Self {
    match (page, self) {
      (Page::Logs, _) => Self::Logs,
      (Page::Log(name), _) => Self::Log(name),
      (Page::Captures, _) => Self::Captures(None),
      (Page::Capture, current) => current.next_capture(report),
      (Page::Records, current) => current.next_records(report),
      (Page::Gaps, Self::Gaps(gaps)) => {
        let next = next_channel(report, Some(&gaps.channel));
        match next == gaps.channel {
          // The one channel: as it is, not looked at again.
          true => Self::Gaps(gaps),
          false => Self::Gaps(Gaps::new(next)),
        }
      }
      (Page::Gaps, _) => Self::Gaps(Gaps::new(next_channel(report, None))),
    }
  }

  fn pane(&self, paths: &Paths, report: &Report) -> Pane<'static> {
    match self {
      Self::Logs => Pane {
        title: "log".into(),
        header: None,
        lines: merge_logs(&paths.logs, &logs(report), 500),
        empty: "(empty: shell runs print to their own windows)",
      },
      Self::Captures(None) => Pane {
        title: "every capture's log, in time order (f for one)".into(),
        header: None,
        lines: merge_logs(&paths.logs, &capture_logs(report), 500),
        empty: "(empty: shell runs print to their own windows)",
      },
      Self::Captures(Some(shown)) => Pane {
        title: format!("{shown}'s log (f again for the next)"),
        header: None,
        lines: match capture_logs(report)
          .into_iter()
          .find(|l| l.shown == *shown)
        {
          Some(log) => merge_logs(&paths.logs, &[log], 500),
          None => Vec::new(),
        },
        empty: "(empty: shell runs print to their own windows)",
      },
      Self::Log(log) => Pane {
        title: format!("{log}.log"),
        header: None,
        lines: tail(&paths.logs.join(format!("{log}.log")), 500)
          .unwrap_or_default(),
        empty: "(empty: shell runs print to their own windows)",
      },
      Self::Records(Some(which)) => Pane {
        title: records_title(which, report),
        header: None,
        lines: capture_tail(&paths.data.join("journal"), which, 500)
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
      Self::Gaps(Gaps {
        channel,
        pane: None,
        ..
      }) => Pane {
        title: format!("gaps in {channel}"),
        header: None,
        lines: Vec::new(),
        empty: "(looking...)",
      },
    }
  }

  /// The first channel's capture's log, or if one's is shown, the next
  /// one's; after the last, every capture's again. The logs each service's
  /// captures shared before each channel had its own are only among every
  /// capture's.
  fn next_capture(&self, report: &Report) -> Self {
    let logs: Vec<Log> = capture_logs(report)
      .into_iter()
      .filter(|log| !SHARED_CAPTURE_LOGS.contains(&log.name.as_str()))
      .collect();
    let next = match self {
      Self::Captures(Some(current)) => logs
        .iter()
        .position(|log| log.shown == *current)
        .map_or(0, |at| at + 1),
      _ => 0,
    };
    Self::Captures(logs.into_iter().nth(next).map(|log| log.shown))
  }

  /// The records of the capture at the top of the captures box, or if a
  /// capture's are shown, the next one's: dgg's, then Kick's, each by
  /// channel, then by instance, so that however the box is sorted,
  /// pressing on comes round to every one.
  fn next_records(&self, report: &Report) -> Self {
    let mut all: Vec<Which> = report
      .captures
      .iter()
      .map(|capture| capture.which())
      .collect();
    let Self::Records(Some(current)) = self else {
      return Self::Records(all.into_iter().next());
    };
    all.sort();
    let next = all.iter().find(|which| *which > current);
    Self::Records(next.or(all.first()).cloned())
  }
}

/// The dgg channel after `current` in `report` (its first, after the last
/// or without one); destiny.gg's if it has none.
fn next_channel(report: &Report, current: Option<&str>) -> String {
  let names: Vec<&str> = report
    .dgg
    .channels
    .iter()
    .map(|c| c.name.as_str())
    .collect();
  let at = current.and_then(|current| names.iter().position(|n| *n == current));
  let next = at.map_or(0, |at| at + 1);
  let next = names.get(next).or(names.first());
  next.map_or(DESTINY, |name| name).to_owned()
}

/// The records page's title: the capture, and what it listens to.
fn records_title(which: &Which, report: &Report) -> String {
  let capture = report.captures.iter().find(|c| c.which() == *which);
  let listening = capture.map_or("?", listening);
  let what = match which.kick {
    true => format!("kick {}, channel {listening}", which.instance),
    false => format!("dgg {}, {listening}", which.instance),
  };
  format!("records: {what} (r again for the next capture)")
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
  let mut boxes = Boxes::default();
  loop {
    let mut report = watcher.gather(paths)?;
    boxes.sort_report(&mut report);
    if let View::Gaps(gaps) = &mut view {
      gaps.refresh(&paths.data, report.dgg.channels.len() > 1);
    }
    let pane = view.pane(paths, &report);

    let until = Instant::now() + interval;
    // Drawn again after each key that moves the boxes, without looking
    // again.
    loop {
      terminal.draw(|frame| draw(frame, &report, &pane, &mut boxes))?;
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
            // A new page starts at its end.
            boxes.scroll[PAGE] = 0;
            break;
          }
          Some(Action::Redraw) => break,
          Some(Action::OpenData) => open(&paths.data),
          Some(Action::Quit) => return Ok(()),
          Some(Action::Minimize) => boxes.minimize(),
          Some(Action::Sort) => {
            boxes.sort(false);
            boxes.sort_report(&mut report);
          }
          Some(Action::Reverse) => {
            boxes.sort(true);
            boxes.sort_report(&mut report);
          }
          None => {}
        },
        KeyCode::Tab => boxes.next(),
        KeyCode::BackTab => boxes.previous(),
        KeyCode::Up => boxes.scroll(true, Some(1)),
        KeyCode::Down => boxes.scroll(false, Some(1)),
        KeyCode::PageUp => boxes.scroll(true, None),
        KeyCode::PageDown => boxes.scroll(false, None),
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
  let mut groups: Vec<String> = KEYS
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
  if let Some(boxes) = groups.last_mut() {
    *boxes = format!("{MOVING}  {boxes}");
  }
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

/// A box's border and title, highlighted if it has the focus.
fn heading(title: &str, focused: bool) -> Block<'_> {
  let border = match focused {
    true => Style::new().fg(Color::Cyan),
    false => Style::new(),
  };
  Block::bordered().border_style(border).title(Span::styled(
    format!(" {title} "),
    border.add_modifier(Modifier::BOLD),
  ))
}

/// How many rows each box is given of `room`, the boxes wanting `lines`
/// lines each (the page, all that's left): each its lines and borders, or
/// minimized, its title line alone. The page gets at least [`PAGE_LEAST`];
/// if the rest don't fit, the tallest give up rows first, down to one
/// line.
fn heights(lines: [usize; 3], minimized: [bool; 4], room: u16) -> [u16; 4] {
  let mut heights = [0; 4];
  for (at, lines) in lines.into_iter().enumerate() {
    heights[at] = match minimized[at] {
      true => 1,
      false => u16::try_from(lines).unwrap_or(u16::MAX).saturating_add(2),
    };
  }
  let page = match minimized[PAGE] {
    true => 1,
    false => PAGE_LEAST,
  };
  let top = |heights: &[u16; 4]| heights[..PAGE].iter().sum::<u16>();
  while top(&heights) + page > room {
    let tallest = (0..PAGE)
      .filter(|&at| !minimized[at] && heights[at] > 3)
      .max_by_key(|&at| heights[at]);
    let Some(tallest) = tallest else { break };
    heights[tallest] -= 1;
  }
  heights[PAGE] = match minimized[PAGE] {
    true => 1,
    false => room.saturating_sub(top(&heights)).max(page),
  };
  heights
}

/// Draws a box above the page: `lines` under `title`, scrolled as `boxes`
/// has it, saying how it's sorted, and where it's scrolled to when they
/// don't all fit: `captures (by health, 1-5 of 9)`.
fn draw_box(
  frame: &mut Frame,
  area: ::ratatui::layout::Rect,
  at: usize,
  title: &str,
  lines: Vec<Line>,
  boxes: &mut Boxes,
) {
  let rows = area.height.saturating_sub(2) as usize;
  let most = lines.len().saturating_sub(rows);
  boxes.most[at] = most;
  boxes.rows[at] = rows;
  let scroll = boxes.scroll[at].min(most);
  boxes.scroll[at] = scroll;
  let keys: &[&str] = match at {
    PROGRAMS => &sort::PROGRAMS,
    CAPTURES => &sort::CAPTURES,
    _ => &sort::JOURNAL,
  };
  let sorted = boxes.order[at].describe(keys);
  let title = if boxes.minimized[at] {
    format!("{title} ({sorted}, minimized, {} lines)", lines.len())
  } else if most > 0 {
    format!(
      "{title} ({sorted}, {}-{} of {})",
      scroll + 1,
      scroll + rows.min(lines.len()),
      lines.len()
    )
  } else {
    format!("{title} ({sorted})")
  };
  let focused = boxes.focus == at;
  let scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
  frame.render_widget(
    Paragraph::new(lines)
      .scroll((scroll, 0))
      .block(heading(&title, focused)),
    area,
  );
}

/// Draws `report`, with `pane` below, the boxes as `boxes` has them.
pub fn draw(
  frame: &mut Frame,
  report: &Report,
  pane: &Pane,
  boxes: &mut Boxes,
) {
  let now = report.now;
  let name = program_width(report);
  let programs: Vec<Line> = if report.programs.is_empty() {
    vec![Line::from("none running".dark_gray())]
  } else {
    report
      .programs
      .iter()
      .map(|program| {
        let up = format::duration(now - program.started as i64 * 1000);
        let from = program_build(program);
        Line::from(vec![
          Span::styled(format!("{:<name$}", program.name), Style::new().bold()),
          Span::raw(format!(
            "  pid {:<6} up {:<8} {:>8} {:>6.1}% cpu  ",
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
  let width = listening_width(report);
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
            "{:<4} {:<30} ",
            service(capture),
            capture.instance
          )),
          Span::styled(
            format!("{:<width$}  ", listening(capture)),
            Style::new().bold(),
          ),
          Span::raw(format!(
            "up {:<8} ",
            format::duration(now - capture.started)
          )),
          Span::styled(
            format!("{:<24} ", health(capture, now)),
            Style::new().fg(color),
          ),
          Span::raw(format!(
            "{:>10} frames {:>9}  ",
            format::count(capture.frames),
            per_minute(capture)
          )),
          Span::styled(
            capture.build.clone().unwrap_or_default(),
            Style::new().dark_gray(),
          ),
        ])
      })
      .collect()
  };
  let journal: Vec<Line> = journal_lines_in(report, boxes.order[JOURNAL])
    .into_iter()
    .map(Line::from)
    .collect();

  let free = report
    .free
    .map(|free| format!("   {} free", format::size(free as f64)))
    .unwrap_or_default();
  let header = Line::from(vec![
    Span::styled(
      format!(" dgg-logger {} ", crate::BUILD),
      Style::new().bold().reversed(),
    ),
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
  let room = frame.area().height.saturating_sub(1 + keys.len() as u16);
  let wanted = [programs.len(), captures.len(), journal.len()];
  let [programs_h, captures_h, journal_h, page_h] =
    heights(wanted, boxes.minimized, room);
  let [
    top,
    programs_area,
    captures_area,
    journal_area,
    page_area,
    _,
    keys_area,
  ] = Layout::vertical([
    Constraint::Length(1),
    Constraint::Length(programs_h),
    Constraint::Length(captures_h),
    Constraint::Length(journal_h),
    Constraint::Length(page_h),
    // What's left with the page minimized.
    Constraint::Min(0),
    Constraint::Length(keys.len() as u16),
  ])
  .areas(frame.area());
  frame.render_widget(header, top);
  draw_box(frame, programs_area, PROGRAMS, "programs", programs, boxes);
  draw_box(frame, captures_area, CAPTURES, "captures", captures, boxes);
  draw_box(frame, journal_area, JOURNAL, "journal", journal, boxes);
  draw_page(frame, page_area, pane, boxes);
  frame.render_widget(Paragraph::new(keys), keys_area);
}

/// Draws `pane` in `area`: its header, then as many of its lines as fit,
/// back from its last as far as `boxes` has it scrolled.
fn draw_page(
  frame: &mut Frame,
  area: ::ratatui::layout::Rect,
  pane: &Pane,
  boxes: &mut Boxes,
) {
  let mut body: Vec<Line> = Vec::new();
  if let Some(header) = &pane.header {
    body.push(Line::from(header.as_str()).bold());
  }
  let rows =
    (area.height.saturating_sub(2) as usize).saturating_sub(body.len());
  let all = pane.lines.len();
  let most = all.saturating_sub(rows);
  boxes.most[PAGE] = most;
  boxes.rows[PAGE] = rows;
  let back = boxes.scroll[PAGE].min(most);
  boxes.scroll[PAGE] = back;
  let shown = &pane.lines[all - rows.min(all) - back..all - back];
  if shown.is_empty() {
    body.push(Line::from(pane.empty.dark_gray()));
  } else {
    body.extend(shown.iter().map(|line| log_line(line)));
  }
  let title = if boxes.minimized[PAGE] {
    format!("{} (minimized)", pane.title)
  } else if back > 0 {
    format!("{} ({back} lines back from the end)", pane.title)
  } else {
    pane.title.clone()
  };
  let focused = boxes.focus == PAGE;
  frame
    .render_widget(Paragraph::new(body).block(heading(&title, focused)), area);
}
