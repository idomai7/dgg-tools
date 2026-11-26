# logger

The logger, codenamed ledgerline: `logger-journal` and `logger`, run as
`dgg-logger` subcommands. See the [overall rules](README.md),
[deploy](deploy.md) for the mesh it runs on, and `deploy/README.md` for
running it. How each part was planned and rolled out is in git history
(tag `planning-full` has the long version of this document).

## Decisions
- **Storage:** an append-only raw journal is the source of truth. SQLite
  (rusqlite, `bundled`) indexes are rebuilt from it at will.
- **Raw, uninterpreted capture:** capture writes frames as received and
  never parses event names, so a service adding or removing events can't
  break the journal. Capture stays tiny and rarely changes.
- **Services:** dgg and Kick side by side, each with its own journals,
  indexes, manifests and routes, the mechanics written once. Each
  implements the `Service` trait (`logger/src/service.rs`); a new service
  is a module and a line in each generic place.
- **Channels:** every service is captured per channel: a list, a journal
  root, an index and a capture process per channel, so one failing leaves
  the others. One code path: no mode capturing every channel in one
  process.
- **Kick, anonymously:** the logger reads Kick's undocumented endpoints
  with no token, login or app, so kick-mod needs only Kick's official API
  and the logger (see [kick-mod](kick-mod.md)).
- **Upgrades never stop capture:** a new capture starts beside the old,
  which hands off once the new one is healthy. Migrations run with capture
  going, by the same handoff. Once every host has moved, the old code goes,
  and a migration's skeleton stays for the next (`logger/src/dgg/migrate.rs`
  says how the last was built, from commit `4869223`).
- **APIs are versioned per service,** `/<service>/v0/...`. `v0` may change
  incompatibly between builds until a stable `v1` is chosen; after that a
  breaking change is a `v2`, served beside it for a while. Builds whose
  routes differ answer each other `404`, which reads as nothing there, so
  they never read a path the wrong way round.
- **Health:** a dgg capture is healthy once its connection has delivered
  `HISTORY`. It used to need a `MSG` too, which a quiet chat such as
  omniliberal.dev may not send for hours, so its upgrades failed. A Kick
  capture is healthy once subscribed.
- **Staggered starts:** an install's bridges and the supervisor's captures
  start 5 s apart, dgg's first (restarts keep their backoff), so the
  services don't see every channel reconnect at once. The install's wait
  for the new captures grows 5 s a channel. Kick's history fills follow
  their capture's connect, so they're spaced too.
- **Log levels:** every line a supervised program logs starts with `INFO`,
  `WARN` or `ERROR` (`common::log`); a line without one is `INFO`. Commands
  run by hand (`cert`, `kick-ids`, `dgg-gaps`) print without levels.

## Journal format
The spec lives in `logger-journal`, each variant's majors in modules of
their own (`record/v0.rs`, `record/kick/`), so formats can coexist.
- **Variants:** each service is a variant, versioned on its own as
  `<variant>.<major>.<minor>`, so one's changes never break another's.
  Versioned like every format (see the [overall rules](README.md)).
- **Current versions:** `journal dgg.0.3` and `kick.0.1` (`kick.0.0`
  still read), `journal-archive dgg.0.2` and `kick.0.0`,
  `journal-manifest dgg.0.2` and `kick.0.0`, `synced-archives` and
  `capture-status` at `dgg.0.2` and `kick.0.0`. Older versions are
  refused.
- **Framing:** the header and every record end at RS LF (RS is 0x1E), so
  data holds LFs and CRs verbatim and a record may span lines. Text
  holding an RS is written base64, its kind suffixed `64`, so an RS only
  ever ends a record. Every line format the logger writes is framed so.
- **Header:** the required fields (format name and version), a lone `;`,
  then informational `key=value` fields, which readers skip if unknown, so
  new keys never change the format. Keys are `[a-z0-9_]+`; values hold no
  space. For example:

  ```text
  journal kick.0.0 ; endpoint=wss://… channel=destiny
  ```

  - `endpoint`: the URL connected to, as the `open` records give it. A
    file only ever holds connections to one endpoint: another starts the
    day's next file, `<instance>.<n>.journal`, numbered in order (readers
    go by number, since by name they sort out of order). Records before
    any `open` go in a file with no endpoint.
  - `channel` (Kick): the channel subscribed to, also given by the path.
- **Records:** `<receive-time> <instance> <connection> <kind> <data>`,
  data verbatim to the record's end. Kinds: `frame` (the text frame),
  `binary` (base64), `open` (the URL), `close` (why); dgg's `fetched`
  (`<source> <timestamp> <nick> <text>`, a backfilled message) and
  `fetchrange` (`<source> <from> <to>`, a span a source returned in full);
  Kick's `history` (`<cursor> <page>`: a page of chat history fetched to
  fill a gap, the cursor asked with or `-`). A Kick frame is Centrifugo's
  raw text, one or more JSON replies separated by LFs. Unknown kinds are
  reported apart from malformed records.
- **Writers** append to a day's file only if its header is the one they'd
  write (version, and endpoint), else start the day's next file, so new
  records never land in an old file.

## Layout
- `data/journal/dgg/<name>/` and `data/journal/kick/<slug>/`: a journal
  root per channel, each with:
  - `live/YYYY-MM-DD/<instance>.journal`: one file per capture instance
    per UTC day, the hot files. Hot paths only ever list `live/`.
  - `archive/YYYY/MM/YYYY-MM-DD.journal.zst`, with a `manifest` a month,
    and `archive/archives.lock`.
  - `control/`: capture statuses (`<instance>.status`), their locks, and
    `synced-archives`.
  - dgg only: `backfill/`, laid out the same, for what was fetched from
    other archives, filed by the messages' day. Nothing in it is
    canonical, so it never mixes with `live/` or `archive/`.
- `data/index-dgg-<name>.sqlite` and `data/index-kick-<slug>.sqlite`.
- **Channel lists,** in the install's root, hand-edited JSONC, each an
  object with a `channels` array, unknown fields refused:
  - `dgg-channels.jsonc`: `name`, `url` (WebSocket) and `origin` (or
    null). A name is the site's domain by convention, `destiny.gg` the
    first: lowercase letters, digits, `-`, `_` and `.`, not starting or
    ending with `.` and no `..`, nor `live`, `archive`, `backfill` or
    `control` (the old layout's directories). With no list, it's
    destiny.gg alone.
  - `kick-channels.jsonc`: `slug`, `channel_id`, `chatroom_id`. Slugs are
    lowercase letters, digits, `-`, `_` and `@` (an app's bot user is
    `@<name>`). `dgg-logger kick-ids <slug>...` looks the ids up
    anonymously and edits the file in place, replacing the channel's
    entry or adding one, so hand edits survive.
  - `install` and the supervisor write a missing list, converting an old
    plain one (`dgg-channels`, `kick-channels`, renamed `.old` once the
    old install has stopped, as its supervisor still reads it).

## capture
- **`dgg-capture --channels <list> --channel <name>`** and
  **`kick-capture --channels <list> --channel <slug>`:** one channel
  each, the list read again every minute. A changed URL, Origin or ids
  reconnects, its connections numbered on (an instance never reuses a
  number); removed, it closes (`removed from the channel list`) and exits
  3 (2 is clap's). A list that can't be read changes nothing. Without
  `--channels`, `dgg-capture`'s list is destiny.gg's alone.
- **Handoff:** each instance heartbeats a status into its channel's
  `control/`, saying what it listens to. An older instance retires once a
  later-started one has a fresh heartbeat (under 5 s) and has been healthy
  for `--handoff-grace` (30 s); its journal ends `close handoff to
  <instance>`. `--keep-running` opts out. A capture holds
  `control/<instance>.live-lock` while it writes.
- **Stale statuses:** a status left by an instance killed or crashed is
  swept away, with a warning, by the newest healthy instance once its
  heartbeat has stopped for 10 minutes. Nothing reads a stale one anyway:
  handoff ignores it, and compact takes it as not writing.
- **Across hosts (dgg):** `dgg-capture --peer <url>` pushes its status to
  the old host's `serve` every second (`PUT
  /dgg/v0/channels/<name>/control/<instance>`), shifted onto that host's
  clock, with `serve`'s control token from `--config-file`; it withdraws
  it on exit. The old instance retires by the local rule. Kick's capture
  has no `--peer` yet.
- **Kick's gaps:** after a blip, the missed messages are fetched from
  Kick's chat history, at most 10 pages, as `ledgerline/<build>`, with
  `kick-api`'s `Web::back_to` (shared with kick-mod), and journaled as
  `history` records. A session Kick ends for good is started again after a
  minute.

## index
- **dgg** (schema 0.6, `logger/schemas/index/`): `messages` (with FTS5
  `messages_fts`), `connections` (open, close and last-seen times, close
  reasons, the oldest timestamp in each `HISTORY`), `fetch_ranges`,
  `unparsed` and `files` (per-file checkpoints).
  - Deduped on `(timestamp, nick, data)`: `HISTORY` is the last 150
    `"MSG {...}"` frames, byte-identical to the live ones. A message seen
    live and in `HISTORY` keeps `source = live` and its earliest receive
    time.
  - Backfilled messages are `source = backfill:<origin>`, matched to
    native ones on timestamp and case-insensitive nick (a whole-second
    timestamp matches anything of that user's within its second). Native
    rows win, a precise timestamp replaces a truncated one, and a nick's
    case comes from native rows, else a source that keeps case.
- **Kick** (schema 0.1, `logger/schemas/index-kick/`; 0.0 would read as a
  new database's `user_version`): messages (`source` `live`, or `history`
  for one only a gap's page had), bans, timeouts and unbans, deletions by
  the deleted message's id, `unparsed` and checkpoints. Rows are keyed by
  Kick's own ids, so captures from several hosts and gap fills merge
  simply, each keeping the earliest receive time; a ban without an id is
  kept as often as seen. Frames are read into events by `kick-api`'s
  `Frame::events`, shared with kick-mod.
- **Both:** each index's paths are relative to its channel's root. A
  file's checkpoint commits with its rows, so an interrupted run resumes
  exactly. Archives are found through the monthly manifests, and each
  checkpoint carries the archive's SHA-256, so a read archive is skipped
  unopened and a re-merged one reread. A live file gone from the journal
  (compacted or not) has its checkpoint and unparsed rows dropped, with a
  warning. Schemas are stored in `user_version` as `major << 16 | minor`;
  an index of another version must be rebuilt.
- **`index --follow`:** a thread per index, picking up new channels at
  each look. Every `--interval` (1 s) it counts the records waiting,
  without decoding them: a round is due once `--batch` (500) wait, no
  sooner than `--min-wait` (60 s) after the last and at the latest
  `--max-wait` (600 s). A summary every `--report` (60 s). `--rebuild`
  rebuilds every index, `--rebuild-dgg` and `--rebuild-kick` one
  service's.

## serve and sync
- **Routes,** for each service:
  - `GET /<service>/v0/channels`: the channels with journals here.
  - `GET .../channels/<name>/manifest` and `.../files/<path>`: a
    channel's files (the `journal-manifest`) and their bytes, honouring
    `Range: bytes=N-` and answering with `Content-Range`.
  - dgg: `PUT`, `DELETE .../channels/<name>/control/<instance>` and `GET
    .../control`: handoff between hosts, for any valid name, since a
    capture's first push may come before its journals.
  - Kick: `GET .../channels/<slug>/users/<id>` and `.../events`, for
    kick-mod (below).
- **Tokens** (`tokens` in serve's config): `read` reads (enough for sync),
  `control` also pushes statuses (which make captures retire), `query`
  reads only Kick's history and live chat. Plain HTTP doesn't hide a
  bearer token: TLS from the mesh's CA, or a tunnel.
- **Sync** (`sync --peer <url>`, `--follow` for a warm standby): for each
  service, the peer's channels (listed here or not; a journal kept is
  never wasted), then each one's manifest.
  - Live files: missing bytes are appended and fsynced, after checking the
    peer sent from where it was asked. Journal files are append-only, so a
    local copy is a prefix and an interrupted sync resumes. A remote file
    shorter than the local one is reported and left alone.
  - Archives are taken whole, checked against their SHA-256, never
    overwriting: for a day with nothing here installed as is, otherwise
    put in `live/<day>/archive-<sha prefix>.journal` for compaction to
    merge. `synced-archives` records what was taken. Sync and compact
    take turns on `archives.lock`.
  - Moving capture between hosts: on the old host, `serve`; on the new,
    `dgg-capture --peer` and `sync --follow`, then `index`; once the old
    capture has retired, a final `sync`.
- **Serving kick-mod:**
  - `users/<id>`: the user's history in the channel as JSON: their
    messages (the newest 500, each with its deletion), and the bans,
    timeouts and unbans on them, with the newest name seen. From the index
    and, in the same snapshot of it, the live files past its checkpoints,
    indexed in memory for the query, so the index's lag never shows and
    nothing counts twice.
  - `events`: live chat as server-sent events, tailing the channel's live
    files every 250 ms, apart from the capture. From now on only; a new
    file is read from its start. Events: `open`, `close`, `event` (Kick's
    name and data, as sent), `history` (a gap's page), each with its
    receive time, instance and connection; an `error` ends the stream. An
    event with a Kick id is passed on once, however many captures' files
    have it. Streams don't count against the 8 requests at once; at most
    16 run.

## backfill
`dgg-backfill --channel <name>` (default `destiny.gg`) fills a channel's
gaps that `HISTORY` didn't cover into its `backfill/` journal, one
`fetched` record a message and a `fetchrange` a page. Both sources hold
destiny.gg's chat alone, so it refuses other channels. A run holds
`backfill/control/<instance>.lock` while it writes, so compact leaves its
files alone. Requests time out after a minute, and a page with no
messages that isn't the source's end stops the run.
- **rustlesearch.dev** (default):

  ```http
  GET https://api-v2.rustlesearch.dev/anon/search?channel=Destinygg&search_after=<ms>
  ```

  - Optional `start_date`/`end_date` (YYYY-MM-DD), `username`, `text`. 100
    a page, newest first, as `{data: {messages: [{channel, text, ts,
    username, searchAfter}]}}`, `searchAfter` the cursor for the next,
    older page. Found via github.com/Fritz-02/rustlesearch-exporter.
  - `ts` is dgg's timestamp to the millisecond (2016-era logs, whole
    seconds). Usernames lowercased, newlines become spaces, no flairs.
    About 5 s behind live; back to at least 2016.
  - 200 requests a day (`Ratelimit-Policy: 200;w=86400`).
- **linkers.ooo** (`--source linkers`):

  ```http
  GET https://linkers.ooo/api/messages?messageType=MSG&endTime=<ISO>&limit=500
  ```

  - Up to 500 a page, newest first, `endTime` inclusive:

    ```js
    {
      messages: [
        {
          id, timestamp, user, message, message_type, user_flairs,
          subscription_tier, ...
        }
      ],
      total, totalCapped, limit, offset
    }
    ```

  - Times are to the second; nicks keep their case; newlines become
    spaces. A full page leaves its oldest second for the next, so no
    second is split.
  - Its own log starts 2026-01-06. `sources=all` adds a static copy of
    the rustle archive up to then: rows with an `a<id>` and a `source` of
    `rustlesearch` (milliseconds) or `overrustle` (seconds), journaled as
    `linkers-rustlesearch` and `linkers-overrustle` (any other source as
    `linkers-<source>`), so each message says which archive it came from.
  - 60 requests a minute per IP (`X-RateLimit-Limit`, `-Remaining`,
    `-Reset` in unix seconds); a logged-in `lk_session` cookie
    (`linkers_session` in backfill's config, valid 7 days) lifts it.
  - Also logs `BAN`, `MUTE`, `SUBSCRIPTION`, `GIFTSUB`, `MASSGIFT` and
    `DONATION`, all with the same fields, which could feed the archive
    tools: a ban or mute's `user` is the moderator, its text `<nick>
    banned by <moderator>`; the others' `user` is `Subscriber`, their text
    chat's announcement. `linkers-api` decodes every kind, keeping an
    unknown one by name.
- overrustlelogs.net is gone (no longer resolves, 2026-11-08).
- **History import:** `--from <date> [--to <date>]` fetches everything
  in the span this source has no `fetchrange` for, newest first. Never
  started automatically. Defaults: 10 s between requests (linkers.ooo's
  whole archive, about 550,000 pages, in about two months), no request
  cap, 20 retries with backoff doubling to 15 min, a progress line every
  10 minutes. Stopping is safe; running again resumes.

## compact
Merges each closed UTC day (ended over `--grace-hours` ago, default 1)
into its archive, then the same for `backfill/`, skipping runs still
going.
- The day's live files and any archive of it are merged byte for byte,
  sorted by receive time. Each distinct record is kept as often as any one
  input has it (dgg does send identical frames twice in a millisecond).
  Torn tails are dropped. Kick's files at `0.0` and `0.1` merge into a
  `0.1` archive whose header keeps the fields every input shares.
- The archive is written to a temp file, fsynced, verified, renamed and
  recorded in the month's manifest, under `archives.lock`; only then are
  the live files deleted, and only those that haven't grown since read.
- `--early` also compacts today, but only files no capture may still
  write (no `.live-lock` held, or for a capture from before the lock, no
  fresh status); the rest join the archive once the day closes. The
  supervisor's first compaction after it starts is `--early`, so an
  upgrade's old files are archived within a minute.

## gaps and migrate
- **`dgg-gaps --channel <name>`:** stretches when no connection was open
  (one that never closed ends at its last record), and whether the next
  connection's `HISTORY` reached back far enough to cover each.
- **`migrate`:** run by the supervisor at start, under the index, compact
  and sync locks, which start only once it succeeds. It logs the build,
  each kind of file in each root with its version (one this build can't
  read gets a line of its own and is left), and each index's schema, and
  warns of anything left of an old layout (`migrate::OLD_LAYOUT`). Its
  log is `migrate.log`. The next format change that needs a migration
  fills it in.

## Open
- Kick's capture, compaction and index run on both hosts; `serve`'s Kick
  endpoints are only exercised by kick-mod's live test, still to do, and
  sync isn't on between the hosts yet.
- `kick-capture` has no `--peer`, so Kick's capture can't move between
  hosts by handoff.
- The newest messages, not yet indexed, could be searched directly (a
  small set, scanned in full), so the index can lag without searches
  missing anything.
- omniliberal.dev has no backfill source: its gaps stay gaps.
- **Left as is by the 2026-11-26 audit** (its fixes are `git log
  pre-audit..`):
  - A backfill page's oldest millisecond may hold more messages than the
    page reached, and those past it are skipped. linkers.ooo's pages
    leave out their oldest second for this; rustlesearch's are to the
    millisecond, so it's rare.
  - A lock file is removed as it's let go, so on Unix a second holder
    could lock a file just unlinked; on Windows, the hosts' OS, the
    removal fails while it's open.
  - Compaction holds a whole day in memory, its records and its archive:
    a few times a busy day's size.
