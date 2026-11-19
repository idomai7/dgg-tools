# logger

The logger: `logger-journal` and `logger`, run as `dgg-logger` subcommands. See the [overall rules](README.md), and [deploy](deploy.md) for the mesh it runs on.

## Decisions
- **Storage:** an append-only raw journal is the source of truth. SQLite (rusqlite, `bundled`) is an index that can be rebuilt from it.
- **Kick journals (planned):** the journal format also covers Kick, for [kick-mod](kick-mod.md) now and a Kick logger later. It stays in `logger-journal`.
  - **Variants:** each platform is a variant of the format, versioned on its own as `<variant>.<major>.<minor>`, so one platform's changes never break the other's. Their events have little in common anyway.
    - Each variant's majors get their own modules in `logger-journal`, like `record/v0.rs` today.
    - Kick starts at `kick.0.0`. dgg's current `journal 0.1` files are dgg's, read as they are until dgg's logger moves to `dgg.0.<minor>`.
  - **Header line:** the required fields, then a lone `;`, then informational `key=value` fields. For example: `journal kick.0.0 ; endpoint=wss://… channel=destiny`.
    - Required: the format name and the variant's version. A new required field changes the format.
    - Informational: redundant information and notes, always written but safe to ignore. Readers skip keys they don't know, so new keys never change the format.
    - `endpoint`: the websocket address for dgg, the socket URL for Kick. It's the same URL the `open` lines record: the one connected to, as given. So a file only ever holds connections to one endpoint, and connecting to a different endpoint starts the next file, `<instance>.<n>.journal`, numbered in sequence.
    - `channel` (Kick): the channel the file's connection is subscribed to, also given by the path.
  - **Layout:** each platform has its own journal root, so sync and compact never mix them. Under Kick's, each channel is a journal root of its own, laid out like dgg's: `<channel>/live/<day>/<instance>.journal`, and so on.
  - **Versions:** dgg's logger isn't changed for now and keeps writing `journal 0.1`. `logger-journal` reads that alongside `kick.0.0`.

## Design
- **Journal layout:**
  - `data/journal/live/YYYY-MM-DD/<instance>.journal`: one file per capture instance per day. These are the hot files.
  - `data/journal/archive/YYYY/MM/YYYY-MM-DD.journal.zst` plus a `manifest.json` per month. These are cold.
  - `data/journal/control/<instance>.status`: heartbeats (`capture-status 0.0`; see `logger/src/control.rs`).
  - `data/journal/backfill/YYYY-MM-DD/<instance>.journal`: what was fetched from other archives, one file per backfill run per day. It's filed by the day of the messages inside, not when they were fetched. Nothing here is canonical, so it never mixes with `live/` or `archive/`.
  - Hot paths only ever list `live/`.
- **Days:** UTC.
- **Line format:** explicit and hand-written, with no serde. The spec lives in the `logger-journal` crate: each major version has its own module (`logger-journal/src/record/v0.rs`) holding its structure, encoding and decoding, so several formats can coexist in the library.
  - Each file starts with `journal <major>.<minor>` (currently `0.1`).
  - Versioned like every format (see the [overall rules](README.md)).
  - Each line is `<receive-time> <instance> <connection> <kind> <data>`, separated by single spaces, with `data` verbatim to the end of the line.
  - Kinds are `frame` (the exact text frame), `binary` (base64), `open` (the URL) and `close` (why).
  - 0.1 adds `fetched` (`<source> <timestamp> <nick> <text>`, one backfilled message) and `fetchrange` (`<source> <from> <to>`, a span the source returned in full).
  - Lines end at LF only, and a CR is ordinary data. Text containing an LF is written as `<kind>64` with the data in base64.
  - Unknown kinds are reported separately from malformed lines.

  Event names are never interpreted at this layer, so dgg adding or removing events can't break the journal.
- **capture:** dgg chat into the journal, and nothing else, so it stays tiny and rarely changes.
  - **In-place upgrade:** start the new binary.
    - An instance is healthy once its current connection has delivered `HISTORY` and a `MSG`.
    - An older instance retires once a later-started one has a fresh heartbeat (under 5 s old) and has been healthy for the grace period (`--handoff-grace`, default 30 s). Its journal ends with `close handoff to <instance>`.
    - `--keep-running` opts out, for deliberate redundancy.
    - Tested live: 34 s of overlap, with 13 MSGs in both journals and none missing from their union.
  - **Across hosts:** the old host runs `serve --control-token …`, and the new instance runs `capture --peer http://old-host:8788 --peer-token <that control token>`.
    - `serve`'s plain `--token` only reads (enough for `sync`); pushing a status needs the control token, since a pushed status makes the host's captures retire.
    - The new instance pushes its status there every second (`PUT /control/<instance>`), shifted onto the old host's clock, and withdraws it on exit.
    - The old instance retires by the same rule as a local handoff, and needs no flags or restart.
    - Use a VPN or SSH tunnel between hosts: the bearer token isn't encrypted over plain HTTP.
- **index:** journal into `data/index.sqlite` (schema in `logger/schemas/index/v<major>.<minor>.sql`, currently 0.4, every past version kept there; stored in `user_version` as `major << 16 | minor`).
  - The dedupe key is `(timestamp, nick, data)`. It was verified with `dgg-api/examples/history_check.rs`: `HISTORY` is a JSON array of the last 150 `"MSG {...}"` frames, byte-identical to the live frames.
  - A message seen both live and in `HISTORY` is kept with `source = live` and the earliest live receive time.
  - Tables: `messages` (with an FTS5 table, `messages_fts`), `connections` (open, close and last-seen times, close reasons, and the oldest timestamp in each connection's `HISTORY`), `unparsed` and `files` (per-file checkpoints).
  - Each file's checkpoint commits in the same transaction as its rows, so an interrupted run resumes exactly where it stopped.
  - `--rebuild` replays the whole journal. An index from another schema version must be rebuilt.
- **serve / sync:** `serve` also lists live files (`GET /manifest`, format `journal-manifest 0.0`) and serves their bytes (`GET /files/live/<day>/<instance>.journal`, honouring `Range: bytes=N-`).
  - `sync --peer <url>` appends whatever remote bytes are missing locally, then fsyncs. Journal files are append-only, so a local copy is always a prefix of the remote file and an interrupted sync just resumes.
  - Archives (listed with their SHA-256) are taken whole and checked, never overwriting anything. One for a day with nothing local is installed as is; otherwise it becomes `live/<day>/archive-<sha prefix>.journal` for compaction to merge. `control/synced-archives` records what was taken, so it isn't fetched again.
  - A remote file shorter than its local copy is reported and left alone.
  - `--follow` keeps a warm standby.
  - **Migration:**
    1. On the old host, run `serve`.
    2. On the new host, start `capture --peer` and `sync --follow`, then run `index`.
    3. When the old capture has retired, run a final `sync`.
  - Tested live: a byte-identical copy, and 0 gaps in the new host's index across both instances.
- **backfill:** fills gaps that `HISTORY` didn't cover, from rustlesearch.dev (default) or linkers.ooo. overrustlelogs.net is gone: the domain no longer resolves (checked 2026-11-08).
  - **API:** `GET https://api-v2.rustlesearch.dev/anon/search?channel=Destinygg&search_after=<ms>`, with optional `start_date`/`end_date` (YYYY-MM-DD), `username` and `text`.
    - It returns `{data: {messages: [{channel, text, ts, username, searchAfter}]}}`: 100 per page, newest first. `searchAfter` is the ms cursor for the next, older page.
    - Found via github.com/Fritz-02/rustlesearch-exporter.
  - **Matches dgg:**
    - `ts` equals dgg's timestamp to the millisecond. 2016-era logs have whole seconds only.
    - Usernames are lowercased, and newlines in text become spaces.
    - There are no flairs or roles.
  - **Freshness and depth:** it runs about 5 s behind live, and goes back to at least 2016.
  - **Rate limit:** 200 requests per day (`Ratelimit-Policy: 200;w=86400`).
  - **Second source, `--source linkers`:** linkers.ooo (`GET https://linkers.ooo/api/messages?messageType=MSG&endTime=<ISO>&limit=500`).
    - It returns `{messages: [{id, timestamp, user, message, message_type, user_flairs, subscription_tier, ...}], total, totalCapped, limit, offset}`, newest first, up to 500 a page. `endTime` is inclusive.
    - Timestamps and `endTime` are truncated to the second. Nicks keep their case, and newlines become spaces. A full page leaves its oldest second for the next page, so no second is ever split.
    - Its own log starts 2026-01-06. Requests use `sources=all`, which adds a static copy of the rustle archive that ends at that date, so it doesn't get around rustlesearch's limit for recent gaps.
    - Rows from that copy have an `a<id>` and a `source` of `rustlesearch` (millisecond timestamps) or `overrustle` (whole seconds). They're journaled as `linkers-rustlesearch` and `linkers-overrustle`, so every message records which archive it really came from. Any other `source` becomes `linkers-<source>`.
    - **Rate limit:** 60 requests per minute per IP (`X-RateLimit-Remaining`). A logged-in `lk_session` cookie (`--linkers-session` / `LINKERS_SESSION`, valid 7 days) removes the limit.
    - Also logs `BAN`, `MUTE`, `SUBSCRIPTION`, `GIFTSUB`, `MASSGIFT` and `DONATION`, which could feed the archive tools.
    - Tested live: it agreed with a 9-minute native capture on all 266 messages (same second and nick); 5 requests filled a 38-minute gap.
  - **Matching:** a backfilled message matches a native one on timestamp plus case-insensitive nick. A whole-second timestamp is taken as truncated and matches anything from that user within its second.
    - Native rows win, and a backfilled row with a precise timestamp replaces a truncated one.
    - Nick casing comes from native rows, failing that from a source that keeps case.
  - Journaled as one `fetched` line per message, plus a `fetchrange` per page, in the run's own `backfill/` files. The index stores them as `source = backfill:<origin>`.
  - **History import:** `backfill --source linkers --from <date> [--to <date>]` fetches everything in that span that this source has no `fetchrange` for yet, newest first. It's built but never started automatically.
    - Defaults: 10 s between requests, which pulls linkers.ooo's whole archive (about 550,000 pages) in about two months. No request cap, 20 retries with backoff (doubling up to 15 min), and a progress line every 10 minutes.
    - Stopping is safe, and re-running resumes from the fetched spans.
    - Smoke-tested on 20 minutes across linkers.ooo's archive boundary: 3 requests, 1,140 `linkers` and 274 `linkers-rustlesearch` messages. A re-run found nothing left to fetch.
  - A run holds `backfill/control/<instance>.lock` while it writes, so `compact` leaves its files alone until it's done.
  - `serve` and `sync` carry `backfill/live/` and `backfill/archive/` like `live/` and `archive/`.
- **compact:** merges each closed UTC day (ended over `--grace-hours` ago, default 1) into `archive/YYYY/MM/YYYY-MM-DD.journal.zst`; then the same for `backfill/`, skipping the files of runs still going.
  - It merges the day's live files and any existing archive for that day, byte for byte, sorted by receive time.
  - Each distinct line is kept as often as any one input has it: dgg does send identical frames twice in one millisecond.
  - Torn tails are dropped.
  - The archive is written to a temp file, fsynced and verified, then renamed and recorded in the monthly manifest. Only after that are the live files deleted, and only those that haven't grown since they were read.
  - Real data: 5 overlapping instances, 1.67 MB, compressed 32x at level 19.

## Status
- **`logger-journal`:** live files, plus a writer and a reader. The `.zst` archives and manifests come with `compact`.
- **`logger capture`:** done, including local handoff.
- **`logger index`:** done for live files, one-shot or `--follow` (polls every `--interval` seconds, default 1; prints a summary every `--report` seconds, default 60).
- **`logger gaps`:** done. It reports stretches when no connection was open (a connection that never closed ends at its last line), and whether the next connection's `HISTORY` reached back far enough to cover each one.
- **`logger backfill`:** fetches uncovered gaps, or with `--from` a span of history, from rustlesearch or linkers.ooo into the journal's `backfill/` directory.
  - It writes one `fetched` line per message and a `fetchrange` per page.
  - It's limited by `--max-requests` (default 50) and stops when the rate limit has fewer than `--reserve` (10) requests left.
  - `--dry-run` lists the gaps it would fetch.
- **Backfill indexing:** done (schema 0.3).
  - Fetched messages are stored with `source = backfill:<source>` and matched on timestamp plus lowercased nick. Native rows win, whichever is indexed first.
  - A fetched nick takes its capitalisation from the same user's native messages.
  - `fetch_ranges` spans mark gaps as filled, and `backfill` skips filled gaps.
  - Live test: after backfill, the index holds exactly a reference capture's 583 messages over the same span. 16 nicks stay lowercase because those users were never seen natively.
- **Cross-host handoff:** done. Tested live: the old instance retired about 38 s after the new one started.
- **`serve` / `sync`:** done. The supervisor's side is in [deploy](deploy.md).
- **`compact`:** done, and `index` reads archives (schema 0.4).
  - Archives are found through the monthly manifests.
  - Each archive's checkpoint carries its SHA-256, so a fully read archive is skipped unopened and a re-merged one is reread from the start.
  - Checkpoints of compacted-away live files are dropped.
  - Real data: the index rebuilt from the archive is identical to one built from the live files (535 messages).
