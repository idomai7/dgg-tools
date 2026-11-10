//! Backfill: fetch messages from other archives into the journal, for gaps
//! that HISTORY didn't cover or for history from before capture began.
//!
//! Each time range is fetched from its end backwards, page by page, until a
//! page reaches back past its start. Messages inside the range are written
//! as `fetched` lines, and each page's span as a `fetchrange` line, to this
//! run's own files in the journal's `backfill/` directory, filed by the
//! messages' day. Nothing there is canonical: matching it against what was
//! captured natively is left to the index, where native messages win.
//!
//! A source may relay messages it got from elsewhere; those are journaled
//! under their [`Message::origin`], so it's always known where a message
//! really came from.

pub mod linkers;
pub mod rustlesearch;

use {
  crate::capture::{now_ms, timestamp},
  ::logger_journal::{
    Writer,
    record::v0::{Body, valid_token},
  },
  ::std::{io, path::PathBuf, time::Duration},
};

/// One message from a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
  /// dgg's timestamp, unix ms.
  pub timestamp: i64,
  pub nick: String,
  pub text: String,
  /// The [`valid_token`] to journal this message under, when the source
  /// relayed it from another archive; otherwise the source's own name.
  pub origin: Option<String>,
}

/// One page of messages, all older than the cursor it was asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
  /// Newest first.
  pub messages: Vec<Message>,
  /// Whether the source has nothing older than these.
  pub exhausted: bool,
  /// Requests the source says are left in its rate-limit window, if it says.
  pub remaining: Option<u64>,
}

/// Somewhere messages can be fetched from.
pub trait Source {
  /// A [`valid_token`] naming the source in the journal.
  fn name(&self) -> &str;

  /// The page of messages just older than `before` (unix ms).
  fn page(
    &self,
    before: i64,
  ) -> impl Future<Output = Result<Page, String>> + Send;
}

#[derive(Clone, Debug)]
pub struct Options {
  pub journal_root: PathBuf,
  /// Instance id for this run's journal files.
  pub instance: String,
  /// Most requests to make in this run.
  pub max_requests: u64,
  /// Stop when the source says fewer requests than this are left.
  pub reserve: u64,
  /// Pause between requests.
  pub delay: Duration,
  /// How many failed requests in a row to retry before giving up. Each
  /// retry waits twice as long as the last, from `delay` (at least 1 s) up
  /// to [`MAX_BACKOFF`].
  pub retries: u32,
  /// How often to print progress, if at all.
  pub report: Option<Duration>,
}

/// The longest wait before retrying a failed request.
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
  pub requests: u64,
  /// Requests that failed and were retried.
  pub retried: u64,
  /// `fetched` lines written.
  pub messages: u64,
  /// Messages dropped because their nick couldn't be journaled.
  pub skipped: u64,
  /// Ranges fetched all the way back to their start.
  pub filled: u64,
  /// Why the run stopped early, if it did.
  pub stopped: Option<String>,
}

/// Fetches each `(from, to)` range of unix ms from `source` into the
/// journal, in the order given.
pub async fn run(
  source: &impl Source,
  ranges: &[(i64, i64)],
  options: &Options,
) -> io::Result<Stats> {
  assert!(valid_token(source.name()), "source name must be a token");
  let mut writer =
    Writer::backfill(options.journal_root.clone(), options.instance.clone())?;
  let mut stats = Stats::default();
  let mut failures = 0;
  let mut last_report = ::std::time::Instant::now();
  'ranges: for &(from, to) in ranges {
    let mut cursor = to;
    loop {
      if stats.requests >= options.max_requests {
        stats.stopped = Some("--max-requests reached".into());
        break 'ranges;
      }
      if stats.requests > 0 {
        ::tokio::time::sleep(options.delay).await;
      }
      if let Some(every) = options.report
        && last_report.elapsed() >= every
      {
        last_report = ::std::time::Instant::now();
        eprintln!(
          "{} requests, {} messages, back to {}",
          stats.requests,
          stats.messages,
          timestamp(cursor)
        );
      }
      stats.requests += 1;
      let page = match source.page(cursor).await {
        Ok(page) => {
          failures = 0;
          page
        }
        Err(e) if failures < options.retries => {
          failures += 1;
          stats.retried += 1;
          let wait = options
            .delay
            .max(Duration::from_secs(1))
            .saturating_mul(1 << (failures - 1).min(20))
            .min(MAX_BACKOFF);
          eprintln!(
            "{}: {e}; retry {failures} of {} in {}s",
            source.name(),
            options.retries,
            wait.as_secs()
          );
          ::tokio::time::sleep(wait).await;
          continue;
        }
        Err(e) => {
          stats.stopped = Some(format!("{}: {e}", source.name()));
          break 'ranges;
        }
      };
      let now = now_ms();
      for message in &page.messages {
        if message.timestamp < from || message.timestamp >= to {
          continue;
        }
        if !valid_token(&message.nick) {
          stats.skipped += 1;
          continue;
        }
        let origin = message.origin.as_deref().unwrap_or(source.name());
        let body = Body::Fetched {
          source: origin.into(),
          timestamp: message.timestamp,
          nick: message.nick.clone(),
          text: message.text.clone(),
        };
        writer.write_on(message.timestamp, now, 1, body)?;
        stats.messages += 1;
      }
      // The page holds everything between its oldest message and the
      // cursor; an exhausted source, everything before the cursor.
      let oldest = page.messages.last().map(|message| message.timestamp);
      let reached = match oldest {
        Some(oldest) if !page.exhausted => oldest,
        _ => i64::MIN,
      };
      let start = reached.max(from).max(0);
      let range = Body::FetchRange {
        source: source.name().into(),
        from: start,
        to: cursor,
      };
      writer.write_on(start, now, 1, range)?;
      writer.sync()?;
      if page.remaining.is_some_and(|left| left < options.reserve) {
        stats.stopped =
          Some(format!("{} rate limit nearly spent", source.name()));
        break 'ranges;
      }
      if reached <= from {
        stats.filled += 1;
        continue 'ranges;
      }
      cursor = reached;
    }
  }
  Ok(stats)
}
