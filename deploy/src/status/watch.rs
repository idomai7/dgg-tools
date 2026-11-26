//! `status --watch`: the report redrawn every second, with a page filling
//! the rest of the screen.
//!
//! The keys are grouped. First, with letters: `l` every log at once, in
//! time order, the page it starts on; `r` the newest records a capture
//! wrote, again for the next capture, round every channel's, and `R` the
//! one before; `f` one channel's capture's log, again for the next, and
//! `F` the one before; Space redraws now, `e` opens the data folder, and
//! `q` (or Esc, or Ctrl-C) quits. Then each program's log, with numbers:
//! `1` the supervisor's, `2` migrate's, `3` every capture's, dgg's and
//! Kick's, in time order, `4` compact's, `5` index's, `6` the gaps in the
//! first dgg channel's capture, looked at again every 30 seconds, again
//! for the next channel's, and `^` (Shift-6) the one before, `7` sync's
//! and `8` serve's. Last, the boxes: Tab and Shift-Tab move the focus
//! between them (the page at the bottom too), and so does a click on one
//! (the screen captures the mouse while it runs), Up, Down, Page Up and Page
//! Down scroll the focused one when it's over-tall, `m` minimizes it to its
//! title line, or restores it, and `s` sorts it by its next key, `S` the
//! other way (see [`sort`]; the page keeps its order). On a narrow screen
//! the keys take more than one line.

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
    crossterm::{
      event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode,
        KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
      },
      execute,
    },
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
      Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
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
  /// One capture's log, the one before.
  CaptureBack,
  Records,
  /// A capture's records, the one before.
  RecordsBack,
  Gaps,
  /// The gaps of the dgg channel before.
  GapsBack,
}

/// The keys, with their names and what they do, in groups: the views
/// across everything and the controls, then each program's log, then the
/// boxes' (after [`MOVING`]). A key with no name does the key before's
/// the other way, and is shown with it (`r/R records`).
const KEYS: [&[(char, &str, Action)]; 3] = [
  &[
    ('l', "logs", Action::Show(Page::Logs)),
    ('r', "records", Action::Show(Page::Records)),
    ('R', "", Action::Show(Page::RecordsBack)),
    ('f', "channel", Action::Show(Page::Capture)),
    ('F', "", Action::Show(Page::CaptureBack)),
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
    ('S', "", Action::Reverse),
  ],
];

/// The keys that aren't characters, before the boxes' group of [`KEYS`].
const MOVING: &str = "tab/click focus  arrows/pgup/pgdn scroll";

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
  /// How far each could scroll, how many lines it showed, and where it
  /// was, as last drawn.
  most: [usize; 4],
  rows: [usize; 4],
  areas: [Rect; 4],
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
      areas: [Rect::default(); 4],
    }
  }
}

impl Boxes {
  /// Does what `mouse` asks: a left click on a box focuses it. Whether
  /// anything changed, so the screen needs drawing again.
  pub fn mouse(&mut self, mouse: MouseEvent) -> bool {
    let at = Position::new(mouse.column, mouse.row);
    let Some(clicked) = self.areas.iter().position(|area| area.contains(at))
    else {
      return false;
    };
    match mouse.kind {
      MouseEventKind::Down(MouseButton::Left) if clicked != self.focus => {
        self.focus = clicked;
        true
      }
      _ => false,
    }
  }

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
        "; 6 for the next channel, ^ the one before"
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
      (Page::Capture, current) => current.next_capture(report, false),
      (Page::CaptureBack, current) => current.next_capture(report, true),
      (Page::Records, current) => current.next_records(report, false),
      (Page::RecordsBack, current) => current.next_records(report, true),
      (Page::Gaps | Page::GapsBack, Self::Gaps(gaps)) => {
        let back = page == Page::GapsBack;
        let next = next_channel(report, Some(&gaps.channel), back);
        match next == gaps.channel {
          // The one channel: as it is, not looked at again.
          true => Self::Gaps(gaps),
          false => Self::Gaps(Gaps::new(next)),
        }
      }
      (Page::Gaps | Page::GapsBack, _) => {
        let back = page == Page::GapsBack;
        Self::Gaps(Gaps::new(next_channel(report, None, back)))
      }
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
        title: format!("{shown}'s log (f for the next, F the one before)"),
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

  /// The next channel's capture's log, or `back`, the one before (see
  /// [`cycle`]). The logs each service's captures shared before each
  /// channel had its own are only among every capture's.
  fn next_capture(&self, report: &Report, back: bool) -> Self {
    let logs: Vec<Log> = capture_logs(report)
      .into_iter()
      .filter(|log| !SHARED_CAPTURE_LOGS.contains(&log.name.as_str()))
      .collect();
    let at = match self {
      Self::Captures(Some(current)) => {
        logs.iter().position(|log| log.shown == *current)
      }
      _ => None,
    };
    let next = cycle(at, logs.len(), back);
    Self::Captures(
      next
        .and_then(|next| logs.into_iter().nth(next))
        .map(|log| log.shown),
    )
  }

  /// The records of the capture at the top of the captures box, or if a
  /// capture's are shown, the next one's, or `back`, the one before:
  /// dgg's, then Kick's, each by channel, then by instance, so that
  /// however the box is sorted, pressing on comes round to every one.
  fn next_records(&self, report: &Report, back: bool) -> Self {
    let mut all: Vec<Which> = report
      .captures
      .iter()
      .map(|capture| capture.which())
      .collect();
    let Self::Records(Some(current)) = self else {
      return Self::Records(all.into_iter().next());
    };
    all.sort();
    Self::Records(next_after(&all, current, back).cloned())
  }
}

/// Going round `len` things and, between the last and the first, every
/// one at once (`None`): the one after `at`, or `back`, the one before.
pub fn cycle(at: Option<usize>, len: usize, back: bool) -> Option<usize> {
  match (at, back) {
    (None, false) => (len > 0).then_some(0),
    (None, true) => len.checked_sub(1),
    (Some(at), false) => (at + 1 < len).then_some(at + 1),
    (Some(at), true) => at.checked_sub(1),
  }
}

/// The first of `sorted` after `current`, or `back`, the last before it;
/// round to the other end, past one.
pub fn next_after<'a, T: Ord>(
  sorted: &'a [T],
  current: &T,
  back: bool,
) -> Option<&'a T> {
  match back {
    false => sorted.iter().find(|t| *t > current).or(sorted.first()),
    true => sorted.iter().rev().find(|t| *t < current).or(sorted.last()),
  }
}

/// The dgg channel after `current` in `report` (its first, after the last
/// or without one), or `back`, before it (its last, before the first or
/// without one); destiny.gg's if it has none.
fn next_channel(report: &Report, current: Option<&str>, back: bool) -> String {
  let names: Vec<&str> = report
    .dgg
    .channels
    .iter()
    .map(|c| c.name.as_str())
    .collect();
  let at = current.and_then(|current| names.iter().position(|n| *n == current));
  let len = names.len();
  let next = match (at, back) {
    (None, false) => 0,
    (None, true) => len.saturating_sub(1),
    (Some(at), false) => (at + 1) % len,
    (Some(at), true) => (at + len - 1) % len,
  };
  names.get(next).map_or(DESTINY, |name| name).to_owned()
}

/// The records page's title: the capture, and what it listens to.
fn records_title(which: &Which, report: &Report) -> String {
  let capture = report.captures.iter().find(|c| c.which() == *which);
  let listening = capture.map_or("?", listening);
  let what = match which.kick {
    true => format!("kick {}, channel {listening}", which.instance),
    false => format!("dgg {}, {listening}", which.instance),
  };
  format!("records: {what} (r for the next capture, R the one before)")
}

/// Runs the screen until the user quits.
pub fn run(paths: &Paths, interval: Duration) -> io::Result<()> {
  let mut watcher = Watcher::new()?;
  let mut terminal = ::ratatui::try_init()?;
  let result = execute!(io::stdout(), EnableMouseCapture)
    .and_then(|()| event_loop(&mut terminal, &mut watcher, paths, interval));
  let _ = execute!(io::stdout(), DisableMouseCapture);
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
  let mut last: Option<Report> = None;
  loop {
    // A look that fails (a file busy as it's replaced, say) keeps the last
    // report up, saying so, and looks again next time.
    let (mut report, failed) = match watcher.gather(paths) {
      Ok(report) => (report, None),
      Err(e) => match last.take() {
        Some(report) => (report, Some(e)),
        None => return Err(e),
      },
    };
    boxes.sort_report(&mut report);
    if let View::Gaps(gaps) = &mut view {
      gaps.refresh(&paths.data, report.dgg.channels.len() > 1);
    }
    let mut pane = view.pane(paths, &report);
    if let Some(e) = failed {
      pane.header = Some(format!(
        "(couldn't look again: {e}; this is as of {})",
        utc_timestamp(report.now)
      ));
    }

    let until = Instant::now() + interval;
    // Drawn again after each key or click that moves the boxes, without
    // looking again.
    let mut redraw = true;
    loop {
      if redraw {
        terminal.draw(|frame| draw(frame, &report, &pane, &mut boxes))?;
      }
      redraw = true;
      let left = until.saturating_duration_since(Instant::now());
      if left.is_zero() || !event::poll(left)? {
        break;
      }
      let key = match event::read()? {
        Event::Key(key) => key,
        // Drawn for only if it changed something: not the mouse moving.
        Event::Mouse(mouse) => {
          redraw = boxes.mouse(mouse);
          continue;
        }
        // A resize, say: redraw.
        _ => break,
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
    last = Some(report);
  }
}

/// What `key` does, if anything.
fn action(key: char) -> Option<Action> {
  KEYS
    .iter()
    .flat_map(|group| group.iter())
    .chain(&UNLISTED)
    .find(|(offered, _, _)| *offered == key)
    .map(|(_, _, action)| *action)
}

/// Keys left out of the keys shown, for room, each told of where it
/// works: `^` (Shift-6), the gaps of the dgg channel before, in the gaps
/// page's title.
const UNLISTED: [(char, &str, Action); 1] =
  [('^', "", Action::Show(Page::GapsBack))];

/// The keys, in their groups, on as many lines `width` wide as they need. A
/// group isn't split across lines, and goes on the first line with room.
pub fn keys(width: u16) -> Vec<String> {
  let mut groups: Vec<String> = KEYS
    .iter()
    .map(|group| {
      group
        .iter()
        .fold(Vec::<String>::new(), |mut shown, (key, name, _)| {
          match (shown.last_mut(), *name) {
            // The key before's, the other way: shown with it, `r/R records`.
            (Some(before), "") => {
              let at = before.find(' ').unwrap_or(before.len());
              before.insert_str(at, &format!("/{key}"));
            }
            _ if *key == ' ' => shown.push(format!("space {name}")),
            _ => shown.push(format!("{key} {name}")),
          }
          shown
        })
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

/// Draws a scrollbar on the right border of the box in `area`, if it has
/// more lines than its `rows` show: `top` the first shown, of `most + 1`
/// it could start at. Its ends say which way there's more (`▲`, `▼`).
fn scrollbar(
  frame: &mut Frame,
  area: ::ratatui::layout::Rect,
  top: usize,
  most: usize,
  rows: usize,
  focused: bool,
) {
  if most == 0 || area.height < 4 {
    return;
  }
  let style = match focused {
    true => Style::new().fg(Color::Cyan),
    false => Style::new(),
  };
  let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
    .begin_symbol(Some(if top > 0 { "▲" } else { "△" }))
    .end_symbol(Some(if top < most { "▼" } else { "▽" }))
    .track_symbol(Some("│"))
    .thumb_symbol("█")
    .style(style);
  let mut state = ScrollbarState::new(most + 1)
    .position(top)
    .viewport_content_length(rows);
  let inside = area.inner(::ratatui::layout::Margin {
    vertical: 1,
    horizontal: 0,
  });
  frame.render_stateful_widget(bar, inside, &mut state);
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
/// line. With any minimized, the rest share the room (see [`shared`]).
pub fn heights(lines: [usize; 3], minimized: [bool; 4], room: u16) -> [u16; 4] {
  if minimized.contains(&true) {
    return shared(lines, minimized, room);
  }
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

/// [`heights`], with a box minimized: its title line alone, and the room
/// left shared evenly between the rest. One wanting less than its share
/// (its lines and borders; the page wants all it can get) takes only that,
/// and the others share what's left, until each takes its share; once
/// every one has all it wants, what's left goes to the shortest, evening
/// them up.
fn shared(lines: [usize; 3], minimized: [bool; 4], room: u16) -> [u16; 4] {
  let wants = |at: usize| match at {
    PAGE => u16::MAX,
    at => u16::try_from(lines[at])
      .unwrap_or(u16::MAX)
      .saturating_add(2),
  };
  let open: Vec<usize> = (0..4).filter(|&at| !minimized[at]).collect();
  let mut heights = [1; 4];
  let mut left = room.saturating_sub((4 - open.len()) as u16);
  // `rows` shared evenly between `among`, the first taking any left over.
  let share = |heights: &mut [u16; 4], among: &[usize], rows: u16| {
    let n = among.len() as u16;
    for (k, &at) in among.iter().enumerate() {
      heights[at] = rows / n + u16::from((k as u16) < rows % n);
    }
  };
  let mut unmet = open.clone();
  while !unmet.is_empty() {
    let even = left / unmet.len() as u16;
    let (met, rest): (Vec<usize>, Vec<usize>) =
      unmet.iter().partition(|&&at| wants(at) <= even);
    if met.is_empty() {
      share(&mut heights, &unmet, left);
      return heights;
    }
    for at in met {
      heights[at] = wants(at);
      left -= wants(at);
    }
    unmet = rest;
  }
  // Every one has all it wants: the shortest grow first, so they're as
  // even as they can be.
  for _ in 0..left {
    let Some(&shortest) = open.iter().min_by_key(|&&at| heights[at]) else {
      break;
    };
    heights[shortest] += 1;
  }
  heights
}

/// Draws a box above the page: `lines` under `title`, scrolled as `boxes`
/// has it, saying how it's sorted, and where it's scrolled to when they
/// don't all fit: `captures (by health, 1-5 of 9)`; minimized, only that:
/// `captures (minimized)`.
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
  boxes.areas[at] = area;
  let scroll = boxes.scroll[at].min(most);
  boxes.scroll[at] = scroll;
  let keys: &[&str] = match at {
    PROGRAMS => &sort::PROGRAMS,
    CAPTURES => &sort::CAPTURES,
    _ => &sort::JOURNAL,
  };
  let sorted = boxes.order[at].describe(keys);
  let title = if boxes.minimized[at] {
    format!("{title} (minimized)")
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
  let top = scroll;
  let scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
  frame.render_widget(
    Paragraph::new(lines)
      .scroll((scroll, 0))
      .block(heading(&title, focused)),
    area,
  );
  if !boxes.minimized[at] {
    scrollbar(frame, area, top, most, rows, focused);
  }
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
  boxes.areas[PAGE] = area;
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
  if !boxes.minimized[PAGE] {
    scrollbar(frame, area, most - back, most, rows, focused);
  }
}
