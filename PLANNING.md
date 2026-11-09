# dgg-tools planning

A Cargo workspace of tools built around destiny.gg chat and its endpoints. Every tool that talks to dgg chat goes through the shared `dgg-chat` crate.

## Layout
The layout is flat: each crate is a direct child of the root.
- `dgg-chat/`: the shared socket client and event types.
- `journal/`: the journal file format, with a writer and a reader.
- `logger/`: the logger as a library: capture, index, serve, sync, backfill, compact and gaps, each with its command-line arguments in `logger::cli`. They run as `dgg-logger` subcommands, so the parts of an install can't be from different builds.
- `deploy/`: `dgg-logger` (the logger's commands, plus install, uninstall, status and supervise) and `dgg-supervisor`, which keeps the logger running as a logon task or a Windows service. See `deploy/README.md`.
- `version-info/`: a build helper embedding version information in the Windows binaries.
- `viewer/`: the read-only chat viewer, formerly dggkick.

## Decisions
- **Stack:** Rust. Node isn't installed on this machine.
  - axum, tokio-tungstenite, reqwest, rustls (ring).
  - Pages are plain JS with no build step.
- **Style:**
  - rustfmt with `max_width = 80` and `tab_spaces = 2` (`rustfmt.toml`).
  - One `use { … }` block per scope, and a `::` prefix on every external crate, `std` included.
  - Tests go in each crate's `tests/` directory, never inline in the source.
- **Deployment:** must stay portable across Windows and Linux.
- **Logger storage:** an append-only raw journal is the source of truth. SQLite (rusqlite, `bundled`) is an index that can be rebuilt from it.
- **Kick:** deferred (see below).

## Roadmap
1. **Logger** (current focus): see the next section.
2. **Read-only viewer:** in the style of omnichat, backfilling from the journal.
3. **Others, not yet ordered:**
   - read-write chat client;
   - embed logger;
   - embed transcriptions;
   - archival;
   - clipping tools;
   - links/attachments viewer (like linkers.ooo);
   - embed stats viewer: when embeds are live, how many viewers, and perhaps which viewers;
   - services dashboard: the status of running services, such as capture instances and their health, `index --follow` and `sync --follow` progress, gaps, and the backfill budget. It could build on `serve`'s `/control` and `/manifest`.
4. **Archives and stats, not yet ordered.** Some can likely be derived from chat events the journal already captures (marked *chat*). The rest need other dgg endpoints or sites, which still have to be probed (marked *probe*).
   - donations: amounts, users, messages, times (*chat*: `DONATION`);
   - gifted subs: gifter, recipients, times (*chat*: `GIFTSUB`/`MASSGIFT`);
   - bans, with their reasons (*chat*: `BAN`/`MUTE`);
   - pinned messages (*chat*: `PIN`);
   - emotes: which are popular, each user's emotes and how often they use them, and rare emotes (*chat*: `MSG` text against the emote list);
    - Halloween and other special-event emotes are counted separately from the regular set. That means keeping a history of the emote list, so it's known which emotes existed when.
    - Greentext (`>` lines) and orangetext are told apart from each other and from emotes. Orangetext is greentext as shown during Halloween events.
      - **Open question:** whether anything in the message marks it as orangetext, or the chat client just restyles greentext during the event. If it's only a client change, tag greentext by whether an event was running at that timestamp, using the emote-list history above or event dates.
   - user activity times (*chat*: `MSG`/`JOIN`/`QUIT`);
   - user subscription statuses (*chat*: `MSG`/`NAMES` carry `subscription` and `features`);
   - TTS messages (*probe*; may arrive as chat events);
   - user bios (*probe*);
   - dgg profile membership cards (*probe*);
   - Vestaboard art (*probe*);
   - tweets (*probe*);
   - the schedule (*probe*);
   - the website's change notes (*probe*).

## Logger design
- **Journal layout:**
  - `data/journal/live/YYYY-MM-DD/<instance>.journal`: one file per capture instance per day. These are the hot files.
  - `data/journal/archive/YYYY/MM/YYYY-MM-DD.journal.zst` plus a `manifest.json` per month. These are cold.
  - `data/journal/control/<instance>.status`: heartbeats (`capture-status 0.0`; see `logger/src/control.rs`).
  - `data/journal/backfill/YYYY-MM-DD/<instance>.journal`: what was fetched from other archives, one file per backfill run per day. It's filed by the day of the messages inside, not when they were fetched. Nothing here is canonical, so it never mixes with `live/` or `archive/`.
  - Hot paths only ever list `live/`.
- **Days:** UTC.
- **Line format:** explicit and hand-written, with no serde. The spec lives in the `journal` crate: each major version has its own module (`journal/src/record/v0.rs`) holding its structure, encoding and decoding, so several formats can coexist in the library.
  - Each file starts with `journal <major>.<minor>` (currently `0.1`).
  - **Versioning, for every format** (journal, `capture-status`, `journal-archive`, `journal-manifest` and the index schema): like cargo's semver.
    - While the major is 0 (pre-release, where we are now), any minor may break. Readers accept only the exact minor they know, and existing files are rewritten rather than read across versions.
    - Major 1 marks production. From then on, minor bumps may only add things old readers skip (new kinds, keys or entries), and anything else is a major bump, which readers without that major's module refuse.
    - Everything was re-versioned from 1.x to 0.x on 2026-11-09, and existing files were rewritten to match.
  - Each line is `<receive-time> <instance> <connection> <kind> <data>`, separated by single spaces, with `data` verbatim to the end of the line.
  - Kinds are `frame` (the exact text frame), `binary` (base64), `open` (the URL) and `close` (why).
  - 0.1 adds `fetched` (`<source> <timestamp> <nick> <text>`, one backfilled message) and `fetchrange` (`<source> <from> <to>`, a span the source returned in full).
  - Lines end at LF only, and a CR is ordinary data. Text containing an LF is written as `<kind>64` with the data in base64.
  - Unknown kinds are reported separately from malformed lines.

  Event names are never interpreted at this layer, so dgg adding or removing events can't break the journal.
- **capture:** dgg-chat into the journal, and nothing else, so it stays tiny and rarely changes.
  - **In-place upgrade:** start the new binary.
    - An instance is healthy once its current connection has delivered `HISTORY` and a `MSG`.
    - An older instance retires once a later-started one has a fresh heartbeat (under 5 s old) and has been healthy for the grace period (`--handoff-grace`, default 30 s). Its journal ends with `close handoff to <instance>`.
    - `--keep-running` opts out, for deliberate redundancy.
    - Tested live: 34 s of overlap, with 13 MSGs in both journals and none missing from their union.
  - **Across hosts:** the old host runs `serve`, and the new instance runs `capture --peer http://old-host:8788 --peer-token …`.
    - The new instance pushes its status there every second (`PUT /control/<instance>`), shifted onto the old host's clock, and withdraws it on exit.
    - The old instance retires by the same rule as a local handoff, and needs no flags or restart.
    - Use a VPN or SSH tunnel between hosts: the bearer token isn't encrypted over plain HTTP.
- **index:** journal into `data/index.sqlite` (schema in `logger/schemas/index/v<major>.<minor>.sql`, currently 0.4, every past version kept there; stored in `user_version` as `major << 16 | minor`).
  - The dedupe key is `(timestamp, nick, data)`. It was verified with `dgg-chat/examples/history_check.rs`: `HISTORY` is a JSON array of the last 150 `"MSG {...}"` frames, byte-identical to the live frames.
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

## destiny.gg chat (from chat-gui: https://github.com/destinygg/chat-gui)
- **Socket:** `wss://chat.destiny.gg/ws`. Each frame is `EVENT {json}`, split on the first space (`assets/chat/js/source.js`).
  - Answer `PING` with `PONG <data>`.
  - The handlers live in `assets/chat/js/chat.js`: MSG, NAMES, HISTORY, JOIN/QUIT, MUTE/BAN, BROADCAST, SUBSCRIPTION/GIFTSUB/MASSGIFT/DONATION, and others.
- **MSG fields:** `nick`, `data`, `timestamp`, `features` (flair names), `roles`, `subscription`, `watching`.
- **Origin:** chat-gui's dev proxy forces `Origin: https://www.destiny.gg`. Anonymous connections work with or without it (tested).
- **Emotes and flairs:**
  - Emotes: https://cdn.destiny.gg/emotes/emotes.json (`prefix`, `image[0].url`) plus `emotes.css`.
  - Flairs: https://cdn.destiny.gg/flairs/flairs.json (`name`, `label`, `priority`, `color`, `hidden`, `image[0].url`) plus `flairs.css`.

## Status
- **Workspace restructure:** done.
  - The dgg probe is at `dgg-chat/examples/probe.rs`.
  - `viewer` is a placeholder stub.
- **`dgg-chat`:** a minimal client (reconnect with backoff, PING/PONG, idle timeout). The typed events and assets are still to do.
- **`journal`:** live files, plus a writer and a reader. The `.zst` archives and manifests come with `compact`.
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
- **`serve` / `sync`:** done.
- **`compact`:** done, and `index` reads archives (schema 0.4).
  - Archives are found through the monthly manifests.
  - Each archive's checkpoint carries its SHA-256, so a fully read archive is skipped unopened and a re-merged one is reread from the start.
  - Checkpoints of compacted-away live files are dropped.
  - Real data: the index rebuilt from the archive is identical to one built from the live files (535 messages).
- **Next:** pick from the roadmap: the read-only viewer, the archives and stats tools, or the services dashboard.

## Deferred: Kick
The Kick probe lives on the `kick` branch (`viewer/examples/kick.rs`, with `.env.example`). It compiles but hasn't been run.
- **Approach:** official webhooks.
  - App token via `POST https://id.kick.com/oauth/token` (`client_credentials`).
  - Channel lookup via `GET /public/v1/channels?slug=` (gives `broadcaster_user_id`).
  - Subscribe with `POST /public/v1/events/subscriptions` (`chat.message.sent`). The webhook URL is set in the app settings.
- **Verification:** RSA PKCS#1 v1.5 SHA-256 over `messageId.timestamp.body`, using the `Kick-Event-*` headers. The public key comes from `/public/v1/public-key`.
- **Payload:** `content`, with emotes inline as `[emote:ID:NAME]`, plus `sender.identity` and `created_at`.
- **Open question:** whether an app token may subscribe to any channel.
- **Fallback:** the unofficial Pusher socket on `chatrooms.{id}.v2`, with key `34bf7a0ff419a2a775b9` on cluster `us2` (changed 2026-10-17).
- **Running the probe needs:**
  - a Kick app with webhooks turned on;
  - a tunnel such as `cloudflared tunnel --url http://localhost:8787`;
  - `.env` filled in from `.env.example`.

  Then, on the `kick` branch, run `cargo run -p viewer --example kick -- <slug>`.
