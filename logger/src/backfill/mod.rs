//! Backfill: fetch messages for gaps that HISTORY didn't cover from another
//! source, into the journal.
//!
//! Each gap is fetched from its end backwards, page by page, until a page
//! reaches back past its start. Messages inside the gap (widened by a
//! margin, since gap edges are local times and message timestamps are
//! dgg's) are written as `fetched` lines, and each page's span as a
//! `fetchrange` line, in this run's own journal file. Matching them against
//! what was captured natively is left to the index.

pub mod rustlesearch;

use {
  crate::{capture::now_ms, gaps::Gap},
  ::journal::{
    Writer,
    record::v1::{Body, valid_token},
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
  /// Instance id for this run's journal file.
  pub instance: String,
  /// How far past each end of a gap to fetch.
  pub margin: Duration,
  /// Most requests to make in this run.
  pub max_requests: u64,
  /// Stop when the source says fewer requests than this are left.
  pub reserve: u64,
  /// Pause between requests.
  pub delay: Duration,
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
  pub requests: u64,
  /// `fetched` lines written.
  pub messages: u64,
  /// Messages dropped because their nick couldn't be journaled.
  pub skipped: u64,
  /// Gaps fetched all the way back to their start.
  pub filled: u64,
  /// Why the run stopped early, if it did.
  pub stopped: Option<String>,
}

/// Fetches `gaps` from `source` into the journal.
pub async fn run(
  source: &impl Source,
  gaps: &[Gap],
  options: &Options,
) -> io::Result<Stats> {
  assert!(valid_token(source.name()), "source name must be a token");
  let mut writer =
    Writer::new(options.journal_root.clone(), options.instance.clone());
  let margin = options.margin.as_millis() as i64;
  let mut stats = Stats::default();
  'gaps: for gap in gaps {
    let (from, to) = (gap.start - margin, gap.end + margin);
    let mut cursor = to;
    loop {
      if stats.requests >= options.max_requests {
        stats.stopped = Some("--max-requests reached".into());
        break 'gaps;
      }
      if stats.requests > 0 {
        ::tokio::time::sleep(options.delay).await;
      }
      stats.requests += 1;
      let page = match source.page(cursor).await {
        Ok(page) => page,
        Err(e) => {
          stats.stopped = Some(format!("{}: {e}", source.name()));
          break 'gaps;
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
        let body = Body::Fetched {
          source: source.name().into(),
          timestamp: message.timestamp,
          nick: message.nick.clone(),
          text: message.text.clone(),
        };
        writer.write(now, 1, body)?;
        stats.messages += 1;
      }
      // The page holds everything between its oldest message and the
      // cursor; an exhausted source, everything before the cursor.
      let oldest = page.messages.last().map(|message| message.timestamp);
      let reached = match oldest {
        Some(oldest) if !page.exhausted => oldest,
        _ => i64::MIN,
      };
      let range = Body::FetchRange {
        source: source.name().into(),
        from: reached.max(from).max(0),
        to: cursor,
      };
      writer.write(now, 1, range)?;
      writer.sync()?;
      if page.remaining.is_some_and(|left| left < options.reserve) {
        stats.stopped =
          Some(format!("{} rate limit nearly spent", source.name()));
        break 'gaps;
      }
      if reached <= from {
        stats.filled += 1;
        continue 'gaps;
      }
      cursor = reached;
    }
  }
  Ok(stats)
}
