# logger

The logger, codenamed ledgerline: `logger-journal` and `logger`, run as
`dgg-logger` subcommands. See the [overall rules](README.md), and
[deploy](deploy.md) for the mesh it runs on.

## Decisions
- **Storage:** an append-only raw journal is the source of truth. SQLite
  (rusqlite, `bundled`) is an index that can be rebuilt from it.
- **Kick journals:** the journal format also covers Kick, for
  [kick-mod](kick-mod.md) now and the logger's Kick capture next (see
  [Kick](#kick)). It stays in `logger-journal`. Written by kick-mod and the
  logger's `kick-capture`; nothing compacts, syncs or indexes Kick's journals
  yet.
  - **Variants:** each platform is a variant of the format, versioned on its own
    as `<variant>.<major>.<minor>`, so one platform's changes never break the
    other's. Their events have little in common anyway.
    - Each variant's majors get their own modules in `logger-journal`, like
      `record/v0.rs` today: Kick's are under `record/kick/`.
    - Kick starts at `kick.0.0`; dgg's is at `dgg.0.3`.
    - `kick.0.0` has dgg's record shape (`<receive-time> <instance>
      <connection> <kind> <data>`, shared code in `record/fields.rs`), with only
      `frame`, `binary`, `open` and `close`. A frame is Centrifugo's raw text:
      one or more JSON replies separated by LFs.
    - **Framing:** each of `kick.0.0`'s records ends at RS LF (RS is 0x1E), so
      its data holds LFs verbatim and a record may span lines. Text data
      holding an RS is written base64, with the kind suffixed `64`, so an RS
      only ever ends a record, and a record with one inside is bad. Valid JSON
      can't hold an RS. The header ends at RS LF too, so every record, the
      header included, can be split on it.
    - **`kick.0.1`** adds `history`: a page of Kick's chat history, fetched
      to fill a gap in the connection's chat. Its data is `<cursor> <page>`:
      the cursor it was asked with (`-` for none) and the page's JSON as
      received. Readers read `kick.0.0` and `kick.0.1`, since kick-mod's
      journals are already at `kick.0.0`.
    - What dgg's readers (index, status) find of Kick's in a dgg root is
      skipped, or recorded as unparsed.
  - **Header line:** the required fields, then a lone `;`, then informational
    `key=value` fields. For example:
    `journal kick.0.0 ; endpoint=wss://… channel=destiny`.
    - Required: the format name and the variant's version. A new required field
      changes the format.
    - Informational: redundant information and notes, always written but safe to
      ignore. Readers skip keys they don't know, so new keys never change the
      format. Keys are `[a-z0-9_]+`; values can't hold a space.
    - `endpoint`: the websocket address for dgg, the socket URL for Kick. It's
      the same URL the `open` records hold: the one connected to, as given. So a
      file only ever holds connections to one endpoint, and connecting to a
      different endpoint starts the next file, `<instance>.<n>.journal`,
      numbered in sequence.
      - Even going back to an earlier endpoint starts the next file, so a
        day's files are in order by number. By name they sort out of order
        (`i.2.journal` before `i.journal`), so readers go by number.
      - Records before any `open` go in a file whose header has no endpoint.
    - `channel` (Kick): the channel the file's connection is subscribed to, also
      given by the path.
  - **Layout:** each platform has its own journal root, so sync and compact
    never mix them. Under Kick's, each channel is a journal root of its own,
    laid out like dgg's: `<channel>/live/<day>/<instance>.journal`, and so on.
  - **dgg moved to RS LF:** dgg's journals, and every other line format the
    logger writes, use Kick's framing: the header and each record or entry
    end at RS LF. Done, and deployed on both hosts.
    - **Versions:** `journal dgg.0.3`, `journal-archive dgg.0.2`,
      `journal-manifest dgg.0.2`, `synced-archives dgg.0.2` and
      `capture-status dgg.0.2`, each header naming its variant. Kick's are
      at `kick.0.0`.
    - **Writers:** a day's file is appended to only if its header is the one
      the writer would write: the same version, and for Kick the same
      endpoint. Otherwise the day's next numbered file is started, so new
      records never land in an old file. (Capture's instances are new each
      run, but backfill's keep their names.)
    - **How it rolled out:** two upgrades, first to records ending at RS LF
      (`journal dgg.0.2`), then to headers ending there too. A `dgg-logger
      migrate`, run by the supervisor at start, rewrote each archive in
      place, validating every record byte for byte against the old file's
      before replacing it, and upgraded the index's offsets to match (schema
      0.4 to 0.6) rather than rebuilding it. Capture never stopped.
      - `capture-status` went in two phases, since an instance hands off
        only to one whose status it can read: first read, then, once every
        instance read it, written.
      - Each path was checked with the real builds against an index rebuilt
        from scratch. It turned up that the index kept `unparsed` rows for
        live files compaction had deleted; it forgets them now.
    - **Older versions are no longer read:** both hosts were migrated, so
      the code for them, and the migration itself, are gone. A file of an
      older version is refused. `migrate` is kept as a skeleton: the
      supervisor still runs it at start, under the index, compact and sync
      locks, and starts those only once it succeeds. For now it only checks,
      and logs what an upgrade touches: first the build, then each kind of
      file in each journal root with its version (a file this build can't
      read gets a line of its own, and is left as it was), then the index's
      schema, which must be 0.6. How the captures hand over is theirs to
      say, in their own logs. The next format change that needs a migration
      fills it in. Its log is `migrate.log`, key `2` in `status --watch`.

## Design
- **Journal layout:** each service's journals in a directory of their own,
  `data/journal/dgg/` and `data/journal/kick/` (see [By service](#by-service)).
  dgg's:
  - `dgg/live/YYYY-MM-DD/<instance>.journal`: one file per capture instance
    per day. These are the hot files.
  - `dgg/archive/YYYY/MM/YYYY-MM-DD.journal.zst` plus a `manifest` per month
    (`journal-archive dgg.0.2`). These are cold.
  - `dgg/control/<instance>.status`: heartbeats (`capture-status dgg.0.2`;
    see `logger/src/control.rs`).
  - `dgg/backfill/live/YYYY-MM-DD/<instance>.journal`: what was fetched from
    other archives, one file per backfill run per day. It's filed by the day of
    the messages inside, not when they were fetched. Nothing here is canonical,
    so it never mixes with `live/` or `archive/`.
  - Hot paths only ever list `live/`.
- **Days:** UTC.
- **Record format:** explicit and hand-written, with no serde. The spec lives in
  the `logger-journal` crate: each major version has its own module
  (`logger-journal/src/record/v0.rs`) holding its structure, encoding and
  decoding, so several formats can coexist in the library.
  - Each file starts with `journal dgg.<major>.<minor>` (currently `0.3`),
    ending at RS LF.
  - Versioned like every format (see the [overall rules](README.md)).
  - Each record is `<receive-time> <instance> <connection> <kind> <data>`,
    separated by single spaces, with `data` verbatim to the end of the
    record.
  - Kinds are `frame` (the exact text frame), `binary` (base64), `open` (the
    URL) and `close` (why).
  - 0.1 adds `fetched` (`<source> <timestamp> <nick> <text>`, one backfilled
    message) and `fetchrange` (`<source> <from> <to>`, a span the source
    returned in full).
  - Records end at RS LF, and LFs and CRs are ordinary data. Text containing
    an RS is written as `<kind>64` with the data in base64. (Up to 0.1, each
    record was a line ending at LF, and text containing an LF went base64.)
  - Unknown kinds are reported separately from malformed records.

  Event names are never interpreted at this layer, so dgg adding or removing
  events can't break the journal.
- **capture:** dgg chat into the journal, and nothing else, so it stays tiny and
  rarely changes.
  - **In-place upgrade:** start the new binary.
    - An instance is healthy once its current connection has delivered `HISTORY`
      and a `MSG`.
    - An older instance retires once a later-started one has a fresh heartbeat
      (under 5 s old) and has been healthy for the grace period
      (`--handoff-grace`, default 30 s). Its journal ends with
      `close handoff to <instance>`.
    - `--keep-running` opts out, for deliberate redundancy.
    - Tested live: 34 s of overlap, with 13 MSGs in both journals and none
      missing from their union.
  - **Across hosts:** the old host's `serve` has a control token
    (`tokens.control` in its config), and the new instance runs
    `capture --peer http://old-host:8788 --config-file <file>`, the file
    holding `{"token": "<that control token>"}`.
    - `serve`'s read token only reads (enough for `sync`); pushing a status
      needs the control token, since a pushed status makes the host's captures
      retire.
    - The new instance pushes its status there every second
      (`PUT /dgg/v0/control/<instance>`), shifted onto the old host's clock, and
      withdraws it on exit.
    - The old instance retires by the same rule as a local handoff, and needs no
      flags or restart.
    - Use a VPN or SSH tunnel between hosts: the bearer token isn't encrypted
      over plain HTTP.
- **index:** journal into `data/index-dgg.sqlite` (schema in
  `logger/schemas/index/v<major>.<minor>.sql`, currently 0.6, every past version
  kept there; stored in `user_version` as `major << 16 | minor`).
  - The dedupe key is `(timestamp, nick, data)`. It was verified with
    `dgg-api/examples/history_check.rs`: `HISTORY` is a JSON array of the last
    150 `"MSG {...}"` frames, byte-identical to the live frames.
  - A message seen both live and in `HISTORY` is kept with `source = live` and
    the earliest live receive time.
  - Tables: `messages` (with an FTS5 table, `messages_fts`), `connections`
    (open, close and last-seen times, close reasons, and the oldest timestamp in
    each connection's `HISTORY`), `unparsed` and `files` (per-file checkpoints).
  - Each file's checkpoint commits in the same transaction as its rows, so an
    interrupted run resumes exactly where it stopped.
  - `--rebuild-dgg` replays dgg's whole journal (`--rebuild` rebuilds both
    indexes). An index from another schema version must be rebuilt.
- **serve / sync:** `serve` also lists live files (`GET /dgg/v0/manifest`,
  format `journal-manifest dgg.0.2`) and serves their bytes
  (`GET /dgg/v0/files/live/<day>/<instance>.journal`, honouring
  `Range: bytes=N-`).
  - `sync --peer <url>` appends whatever remote bytes are missing locally, then
    fsyncs. Journal files are append-only, so a local copy is always a prefix of
    the remote file and an interrupted sync just resumes.
  - Archives (listed with their SHA-256) are taken whole and checked, never
    overwriting anything. One for a day with nothing local is installed as is;
    otherwise it becomes `live/<day>/archive-<sha prefix>.journal` for
    compaction to merge. `control/synced-archives` records what was taken, so it
    isn't fetched again.
  - A remote file shorter than its local copy is reported and left alone.
  - `--follow` keeps a warm standby.
  - **Migration:**
    1. On the old host, run `serve`.
    2. On the new host, start `capture --peer` and `sync --follow`, then run
       `index`.
    3. When the old capture has retired, run a final `sync`.
  - Tested live: a byte-identical copy, and 0 gaps in the new host's index
    across both instances.
- **backfill:** fills gaps that `HISTORY` didn't cover, from rustlesearch.dev
  (default) or linkers.ooo. overrustlelogs.net is gone: the domain no longer
  resolves (checked 2026-11-08).
  - **API:**
    ```http
    GET https://api-v2.rustlesearch.dev/anon/search?channel=Destinygg&search_after=<ms>
    ```
    with optional `start_date`/`end_date` (YYYY-MM-DD), `username` and `text`.
    - It returns 100 per page, newest first:
      ```js
      {data: {messages: [{channel, text, ts, username, searchAfter}]}}
      ```
      `searchAfter` is the ms cursor for the next, older page.
    - Found via github.com/Fritz-02/rustlesearch-exporter.
  - **Matches dgg:**
    - `ts` equals dgg's timestamp to the millisecond. 2016-era logs have whole
      seconds only.
    - Usernames are lowercased, and newlines in text become spaces.
    - There are no flairs or roles.
  - **Freshness and depth:** it runs about 5 s behind live, and goes back to at
    least 2016.
  - **Rate limit:** 200 requests per day (`Ratelimit-Policy: 200;w=86400`).
  - **Second source, `--source linkers`:** linkers.ooo:
    ```http
    GET https://linkers.ooo/api/messages?messageType=MSG&endTime=<ISO>&limit=500
    ```
    - It returns up to 500 a page, newest first:
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
      `endTime` is inclusive.
    - Timestamps and `endTime` are truncated to the second. Nicks keep their
      case, and newlines become spaces. A full page leaves its oldest second for
      the next page, so no second is ever split.
    - Its own log starts 2026-01-06. Requests use `sources=all`, which adds a
      static copy of the rustle archive that ends at that date, so it doesn't
      get around rustlesearch's limit for recent gaps.
    - Rows from that copy have an `a<id>` and a `source` of `rustlesearch`
      (millisecond timestamps) or `overrustle` (whole seconds). They're
      journaled as `linkers-rustlesearch` and `linkers-overrustle`, so every
      message records which archive it really came from. Any other `source`
      becomes `linkers-<source>`.
    - **Rate limit:** 60 requests per minute per IP (`X-RateLimit-Remaining`). A
      logged-in `lk_session` cookie (`linkers_session` in backfill's config,
      valid 7 days) removes the limit.
    - Also logs `BAN`, `MUTE`, `SUBSCRIPTION`, `GIFTSUB`, `MASSGIFT` and
      `DONATION`, which could feed the archive tools. Every kind has the same
      fields (checked 2026-11-24). A ban or mute's `user` is the moderator,
      and its text `<nick> banned by <moderator>`; the others' `user` is
      `Subscriber`, and their text chat's announcement.
    - Its limit's headers are `X-RateLimit-Limit`, `-Remaining` and `-Reset`
      (unix seconds).
    - Tested live: it agreed with a 9-minute native capture on all 266 messages
      (same second and nick); 5 requests filled a 38-minute gap.
  - **Matching:** a backfilled message matches a native one on timestamp plus
    case-insensitive nick. A whole-second timestamp is taken as truncated and
    matches anything from that user within its second.
    - Native rows win, and a backfilled row with a precise timestamp replaces a
      truncated one.
    - Nick casing comes from native rows, failing that from a source that keeps
      case.
  - Journaled as one `fetched` record per message, plus a `fetchrange` per page,
    in the run's own `backfill/` files. The index stores them as
    `source = backfill:<origin>`.
  - **History import:** `backfill --source linkers --from <date> [--to <date>]`
    fetches everything in that span that this source has no `fetchrange` for
    yet, newest first. It's built but never started automatically.
    - Defaults: 10 s between requests, which pulls linkers.ooo's whole archive
      (about 550,000 pages) in about two months. No request cap, 20 retries with
      backoff (doubling up to 15 min), and a progress line every 10 minutes.
    - Stopping is safe, and re-running resumes from the fetched spans.
    - Smoke-tested on 20 minutes across linkers.ooo's archive boundary: 3
      requests, 1,140 `linkers` and 274 `linkers-rustlesearch` messages. A
      re-run found nothing left to fetch.
  - **API crates:** rustlesearch.dev and linkers.ooo have their own crates,
    `rustlesearch-api` and `linkers-api` (see the [overall rules](README.md)),
    split out of `logger/src/backfill/`. Backfill keeps its `Source`s, turning
    their answers into its pages, and the journaling.
    - `linkers-api` decodes every kind of message linkers.ooo logs, not only
      `MSG`, for the archive tools; an unknown kind is kept by name. It
      doesn't parse their text.
    - Requests name the logger `ledgerline/<build>`.
  - A run holds `backfill/control/<instance>.lock` while it writes, so `compact`
    leaves its files alone until it's done.
  - `serve` and `sync` carry `backfill/live/` and `backfill/archive/` like
    `live/` and `archive/`.
- **compact:** merges each closed UTC day (ended over `--grace-hours` ago,
  default 1) into `archive/YYYY/MM/YYYY-MM-DD.journal.zst`; then the same for
  `backfill/`, skipping the files of runs still going.
  - It merges the day's live files and any existing archive for that day, byte
    for byte, sorted by receive time.
  - Each distinct record is kept as often as any one input has it: dgg does send
    identical frames twice in one millisecond.
  - Torn tails are dropped.
  - The archive is written to a temp file, fsynced and verified, then renamed
    and recorded in the monthly manifest. Only after that are the live files
    deleted, and only those that haven't grown since they were read.
  - Real data: 5 overlapping instances, 1.67 MB, compressed 32x at level 19.
  - `--early` also compacts days still open, but only the files no capture
    may still write: a capture holds `control/<instance>.live-lock` while it
    writes (apart from backfill's `.lock`, as it doesn't hold back closed
    days), and one from before the lock counts while its status is fresh.
    The rest join that archive once the day is closed, as after a late
    `sync`. The supervisor's first compaction after start is `--early`, so
    an upgrade's old capture's files are archived within a minute.
    (Added 2026-11-25.)

## Kick
The logger captures Kick too, so kick-mod needs nothing but Kick's official
API and the logger (see [kick-mod](kick-mod.md)). The logger reads Kick's
undocumented endpoints anonymously: no token, no login, no app. Planned
2026-11-24.
- **Process:** `dgg-logger kick-capture`, a program of its own beside dgg's
  `capture`, so that stays small and untouched. The supervisor runs it when
  there are Kick channels to capture.
- **Layout:** `data/journal/kick/<slug>/`, one journal root per channel (as
  kick-mod's are), each with its own `live/`, `archive/` and `control/`.
- **Channels:** a `kick-channels` file in the install, like `nodes`: a line
  per channel, its slug, channel id and chatroom id.
  - `dgg-logger kick-ids <slug>...` looks the ids up with the unofficial
    channel lookup, anonymously, and writes them there. Asking the lookup
    is `kick-api`'s `lookup` feature: with reqwest, as `ledgerline/<build>`
    (with curl until 2026-11-25).
  - Slugs are lowercase letters, digits, `-` and `_`, so each is also a
    directory's name. Hand edits survive `kick-ids`: it changes only the
    channel's own line.
- **Capture:** a Centrifugo connection per channel, each journaled as kick-mod
  does now. `kick-capture --channels <list>` takes the list's path. A session
  Kick stops for good is started again after a minute, its connections
  numbered on from the last, so an instance never reuses a number.
  - **Following the list** (2026-11-25): it's read again every minute. A
    channel added is captured from then on; one removed stops, its open
    connection closed (`removed from the channel list`); one whose ids
    changed stops and starts again with them, its connections numbered on.
    A list that can't be read leaves the channels as they were.
  - **Handoff:** as dgg's capture: heartbeats in `control/`, and an older
    instance retires once a newer one has been healthy for the grace period.
    Healthy means subscribed on every channel.
    - The status is written in every channel's `control/`, as
      `capture-status kick.0.0` (dgg's entries, Kick's variant).
    - An instance retires on a successor seen in any of its channels.
    - Not yet across hosts: `kick-capture` has no `--peer`.
  - **Gaps:** a blip's messages are filled from Kick's chat history, at most
    10 pages, with `User-Agent: ledgerline/<build>`. The paging is
    `kick-api`'s `Web::back_to`, shared with kick-mod.
- **Compact and sync:** both cover Kick's roots too, every channel with a
  directory under `kick/`, listed or not.
  - Compaction's merge works for Kick's records as they are: each `open`
    names its endpoint. Files at `kick.0.0` and `0.1` merge into a `0.1`
    archive, whose header keeps the fields every input shares (`channel`,
    and `endpoint` if the day had only one). Its manifest is `journal-archive
    kick.0.0`.
  - `serve` lists Kick's journals in a manifest of their own, `GET
    /kick/v0/manifest` (`journal-manifest kick.0.0`), with paths relative
    to `kick/`, `<slug>/live/...` and `<slug>/archive/...`, and serves
    those files at `/kick/v0/files/...`. A peer without Kick's manifest
    (`404`) has none.
  - Each channel's root has its own `control/synced-archives`, as
    `synced-archives kick.0.0`.
- **Index:** `index-kick.sqlite`, separate from dgg's, with schema versions of
  its own (`logger/schemas/index-kick/`, from 0.1, since 0.0 would read as a
  new database's `user_version`).
  - Rows are keyed by Kick's own ids (the uuids of messages, bans and
    deletions), so captures from several hosts and gap fills merge simply.
    Each keeps the earliest receive time.
  - Tables: messages (`source` `live`, or `history` for one only a gap's
    page had); bans, timeouts and unbans; deletions, by the deleted
    message's id; `unparsed`; and per-file checkpoints, as dgg's. Kick's
    ban, unban and deletion ids are optional: a ban without one is kept as
    often as it's seen.
  - `dgg-logger index` keeps both: Kick's follows on a thread of its own, at
    the same pace, so either failing for good leaves the other. `--rebuild`
    rebuilds both, `--rebuild-dgg` and `--rebuild-kick` only one. Its log
    lines start `kick: `.
  - Files are named by their paths in `kick/`, `<slug>/live/...`.
  - Frames are read into events by `kick-api`'s `Frame::events`, shared
    with kick-mod.
  - `migrate` checks its schema too, and `status` shows its size and last
    write.
- **Serving kick-mod:**
  - `GET /kick/v0/channels/<slug>/users/<id>`: a user's history in the
    channel, as JSON: their messages (the newest 500, each with its
    deletion), and the bans, timeouts and unbans on them, with the newest
    name seen.
    - From the Kick index, and from what it hasn't reached yet: the
      channel's live files past its checkpoints, indexed in memory for the
      query (the same code, into an in-memory database). Both are read in
      one snapshot of the index, so a round finishing meanwhile can't count
      anything twice, and the index's lag never shows.
  - `GET /kick/v0/channels/<slug>/events`: live chat as server-sent events, by
    tailing the channel's live files every 250 ms, apart from the capture
    process.
    - From now on: nothing written before is replayed. A new file (another
      instance's, or the next day's) is read from its start.
    - Events are `open`, `close`, `event` (Kick's event name and data, as
      Kick sent them, for kick-mod to decode with `kick-api`) and `history`
      (a gap's page, as sent), each with its receive time, instance and
      connection. An `error` ends the stream.
    - An event Kick gave an id (a message, deletion, ban or unban) is passed
      on once, however many captures' files have it.
    - Streams don't count against the 8 requests at once; at most 16 run.
  - A third token, a query token (`tokens.query` in `serve`'s config),
    reads only Kick's history and live chat. The read and control tokens
    read them too. kick-mod holds only the query token, and the mesh CA's
    certificate for TLS.
  - kick-mod is pointed at one node's `serve`, with others to fall back on.
- **Steps,** each a pause for review:
  1. The channel list and `kick-ids`.
  2. `kick-capture`, with gap filling.
  3. The supervisor and `status`.
  4. Compact and sync.
  5. The Kick index.
  6. The query and live endpoints.
  7. kick-mod switches over (see [kick-mod](kick-mod.md)).

## By service
dgg and Kick side by side, each a service with its own journals, index,
manifest and routes, and the shared mechanics written once. Nothing syncs
yet, so old peers need no compatibility. Planned 2026-11-24.
- **Code:** a module per service, `dgg/` and `kick/`, each implementing a
  `Service` trait: its name (the route segment, the directory and the
  formats' variant), its formats' versions, its index (with its own
  stats), its journal roots, which paths it serves, and routes of its own.
  Manifests, sync, compaction and index pacing are generic over it, called
  for each service in turn, so a new service is a module and a line in
  each.
- **Index flags:** `--rebuild` rebuilds both indexes, `--rebuild-dgg` and
  `--rebuild-kick` only one.
- **Routes:** `/<service>/<api>/manifest`, `/<service>/<api>/files/<path>`
  and `/<service>/<api>/control/<instance>`, each service's paths relative
  to its own root. Kick's queries move to `/kick/<api>/channels/<slug>/...`,
  so a channel named `files` or `manifest` can't collide. A peer without a
  service answers 404, which reads as nothing there; an old build never
  asks for a service it doesn't know.
  - `<api>` is each service's API's version (`Service::API`), `v0` for
    both. `v0` is unstable: it may change incompatibly from one build to
    the next, until a stable `v1` is decided on. A `v1` would then only
    change compatibly, and a breaking change would be a `v2`, served beside
    it for a while. (Added 2026-11-24, after the rest.)
- **Layout:** dgg's journals move into `data/journal/dgg/`, beside `kick/`,
  and `index.sqlite` becomes `index-dgg.sqlite`. Paths inside a root don't
  change (`live/<day>/<instance>.journal`), so the index's checkpoints and
  every format's version stay as they are. Kick's index stores paths
  relative to `kick/` instead (it has never run on a host).
- **Migration, with capture running:** one release, by the handoffs used
  before. The status format isn't changing, only where it's kept.
  - The new capture writes under `dgg/`, and while the old `control/`
    exists it heartbeats there too, so the old instance retires to it as
    usual. Pushing across hosts tries `/dgg/v0/control/<instance>`, then the
    old `/control/<instance>` on a 404.
  - `migrate`, at the supervisor's start under the index, compact and sync
    locks, moves `archive/` (with its manifests and synced-archives),
    `backfill/`, and every live file no running instance is writing (its
    status missing, or stale by the handoff rule) into `dgg/`, and renames
    the index. Each move is a rename within the volume; a file already at
    its destination is refused, as `install`'s merge does.
  - A running old instance's files stay where they are (moving an open
    file isn't reliable on Windows). Index, compact, serve and sync read
    dgg's live files from both places, and compaction merges the old ones
    into `dgg/archive/` once their day is over. The next `migrate` removes
    the empty old directories, and the old `control/` once nothing in it
    is fresh.
  - Tested locally with both builds, as before: the current build
    capturing, the new one installed over it; the handoff, what moved and
    what stayed, the index against one rebuilt from scratch, and
    compaction draining the old files.
- **Removing the old code:** once every host has migrated and compacted
  what its old capture left (since `compact --early`, a minute after the
  supervisor starts; before, the day after the rollout), a later release
  drops reading the old places (`Service::legacy`, `Root::legacy`,
  `service::locate`), the second heartbeat (`Config::legacy_control`, and
  serve's `legacy_control`), the `/control` fallback, and `dgg::migrate`.
  `migrate` then only checks that the old layout is gone, and logs anything
  left (`migrate::OLD_LAYOUT`).
- **Steps,** each a pause for review:
  1. The index flags.
  2. The `Service` trait and `dgg/`, moving code without changing
     behaviour.
  3. Manifests, sync, compaction and index pacing generic over it, and the
     routes (the manifests' paths and the file routes change together).
  4. The layout and its migration.
  5. Docs.
  6. Removing the old code, after the rollout.
- **Status:** done. Steps 1 to 5 on 2026-11-24, rolled out to both hosts
  the same day; step 6 on 2026-11-25, once both had migrated and run
  `compact --early`.
  - `Service` (`logger/src/service.rs`): its name, variant, index file and
    stats, its directory, its journal roots, and how a path splits into a
    root's prefix and the rest. dgg's is in `dgg/` (capture, index, gaps,
    backfill), Kick's in `kick/`.
    Capture's handoff machinery (`capture.rs`) and the index's opening and
    pacing (`index.rs`) are shared.
  - Not as planned: Kick has no `/kick/v0/control` (`kick-capture` has no
    `--peer` yet), so only dgg's control routes moved.
  - The move was `dgg::migrate`, called by `migrate`, until step 6. Each
    step was safe to repeat; a file at both paths, or an index at both
    names, failed the migration for a person to sort out.
  - In an install, the bridge starts before the old capture hands off, and
    `migrate` runs only once the old supervisor has stopped, so nothing was
    left being written on either host: both moved everything at once.

## A capture per Kick channel
Planned 2026-11-25. Each step a pause for review. All four, and the
captures saying what they listen to, upgraded on both hosts 2026-11-25.
1. **Names by service:** dgg's own commands name their service, as Kick's
   do: `capture` becomes `dgg-capture`, `backfill` `dgg-backfill` and
   `gaps` `dgg-gaps`, with no aliases. `capture.log` becomes
   `dgg-capture.log` (`install` renames an old one). `install` still
   finds an older build's `dgg-logger capture` to wait on its handoff.
   Done 2026-11-25 (`b9651ea`).
2. **A process per Kick channel:** `kick-capture --channels <list>
   --channel <slug>` captures one channel, so one failing leaves the
   others. It reads the list again every minute: changed ids reconnect it,
   its connections numbered on; removed, it exits as such. The supervisor
   runs one per listed channel, each restarted on its own, reading the
   list every minute to start the added. All log to `kick-capture.log`,
   each line naming its channel. Capturing every channel in one process
   goes: one code path. Done 2026-11-25 (`8ec7447`): a capture that ended
   cleanly (handed off, or removed) starts again only once its channel
   has left the list and come back. `install` already starts a bridge per
   channel and waits on them together.
3. **`install` waits for every Kick channel:** a bridge per channel,
   waited on together, and the new install's Kick captures waited on too,
   not only dgg's. A channel not healthy in time is warned of, by name:
   dgg's capture comes first. Done 2026-11-25 (`8ec7447`, `a424234`).
4. **One captures box in `status`:** dgg's and Kick's captures together,
   each coloured by health, and after its instance what it listens to:
   dgg's WebSocket URL (from its live file's header) and Kick's channel
   (from the `control/` its status is in). Kick's frames counted as
   dgg's. Kick's channels' journal lines join the journal box, and the
   kick box goes. The programs box names each `kick-capture`'s channel.
   Done 2026-11-25 (`4b050f7`). Each capture's status now says what it
   listens to (a `listening` entry, `d8a5eeb`), which status shows; for
   a capture from before, the live files' last `open`. dgg's live files
   name their endpoint in the header too, as Kick's do (`641425f`).

## The status screen, and dgg channels
Planned 2026-11-25. Each step a pause for review.
1. **Programs and captures:** the programs box's columns aligned to the
   widest name. Each capture prefixed by its service, `dgg` or `kick`.
   Captures sorted unhealthy first (stale, then connecting), then by
   frames a minute, over about the last minute of snapshots. Done: the
   rate is shown too (`12/min`), blank until a second snapshot.
2. **Records:** `r` cycles through every capture, Kick's too, and the
   page's title names what it listens to (the Kick channel, or dgg's
   endpoint). Done: the first press shows the capture at the top of the
   box, and pressing on goes round dgg's, then Kick's by channel.
3. **Log pages:** `3` capture shows every capture's log, dgg's and
   Kick's, in time order, each line naming its channel; a key narrows it
   to one channel, cycling. `8` goes. The rest (index, migrate, compact,
   sync) already hold both services' lines. Done: each Kick channel's
   capture logs to `kick-capture.<slug>.log`, so a channel's lines are
   shown apart exactly; `f` is the key, and `serve` moved to `8`.
4. **Boxes:** Tab and Shift-Tab move focus between the boxes (the log
   page too), its border highlighted. Up, Down, Page Up and Page Down
   scroll the focused one when it's over-tall; `m` minimizes it to its
   title line, or restores it, and the others take the room. Done: the
   page has the focus to start with, and keeps at least six rows unless
   it's minimized; boxes that don't fit give up rows, the tallest first.
5. **dgg channels:** dgg captured per channel, as Kick is: a
   `dgg-channels` list (name and WebSocket URL), a journal root per
   channel (`journal/dgg/<name>/`), a `dgg-capture` per channel, and an
   index per channel (`index-dgg-<name>.sqlite`). Today's journal and
   index move into the first channel's, as the move by service did.
   Planned in detail in [dgg channels](#dgg-channels).

## dgg channels
Planned 2026-11-25, to be reviewed before any of it is built. dgg captured
per channel, as Kick is, each channel a chat speaking dgg's protocol at a
WebSocket URL of its own. Each step a pause for review.
- **The list:** `dgg-channels` in the install's root, beside
  `kick-channels`, a line per channel, edited by hand:

  ```text
  # <name> <WebSocket URL> [<Origin header>, or - for none]
  destiny.gg wss://chat.destiny.gg/ws https://www.destiny.gg
  ```

  A name is the site's domain by convention, `destiny.gg` the first
  (decided 2026-11-25): lowercase letters, digits, `-`, `_` and `.`, not
  starting or ending with `.` and with no `..`, so it's a safe directory
  name on Windows too and never `.` or `..`. Nor is it one of the
  directories of the old layout (`live`, `archive`, `backfill`,
  `control`), so a name is never mistaken for one while the move is under
  way. With no list, it's this one line: a host that never
  writes one keeps capturing destiny.gg as now. `install` moves the list
  with the root, as `kick-channels`.
- **Layout:** each channel's journals in `journal/dgg/<name>/`, laid out
  as dgg's directory is now: `live/`, `archive/`, `control/` (statuses
  and `synced-archives`), and its backfill journal in `backfill/`. Paths
  inside a channel's directory are as they are now in `dgg/`, so every
  format's version stays as it is.
- **Index:** one per channel, `index-dgg-<name>.sqlite`. Its paths are
  relative to the channel's directory, so today's index becomes
  destiny.gg's by a rename, with no rebuild and no schema change. Its
  deduplication (timestamp and nick), connections, gaps and fetched
  ranges stay those of one chat, as they are now. `index --follow` keeps
  each channel's on its own, picking up a channel added to the list at its
  next look. `--rebuild-dgg` rebuilds every channel's.
  - Kick's index is split per channel too, `index-kick-<slug>.sqlite`,
    its paths relative to the channel's directory, so every service's
    indexes are alike. Its rows are already keyed by Kick's own ids, so
    nothing about them changes; each channel's is rebuilt from its
    journal once, and `index-kick.sqlite` deleted. Its own step, after
    dgg's (decided 2026-11-25).
- **Routes,** both services alike, every channel's under `channels/` as
  Kick's queries are now:
  - `GET /<service>/v0/channels`: the channels with journals here, a
    line each.
  - `GET /<service>/v0/channels/<name>/manifest` and
    `.../files/<path>`: a channel's manifest and files, paths relative to
    its directory (dgg's as now; Kick's lose the `<slug>/` they start with
    now).
  - `PUT` and `DELETE /dgg/v0/channels/<name>/control/<instance>`, and
    `GET /dgg/v0/channels/<name>/control`: a channel's handoff between
    hosts, taken for any channel's name, journals here or not, since a
    capture's first push may come before them.
  - `/<service>/v0/manifest`, `/files` and `/dgg/v0/control` go. A peer on
    the old build answers the new routes `404` (nothing there), and the
    new build answers the old ones `404`, so hosts on different builds
    don't sync until both are upgraded, and then catch up: no path is
    ever read the wrong way round.
- **Sync:** for each service, the peer's channels, then each channel's
  manifest and files, pulled into the channel's directory here. A channel
  the peer has and this host doesn't list is pulled too, as Kick's are
  now (a journal kept is never wasted; nothing captures it here).
- **Compact:** each channel's root and its backfill root, as dgg's are
  now. Nothing else changes.
- **Capture:** `dgg-capture --channels <list> --channel <name>`, as
  `kick-capture`: it reads the list every minute; a changed URL or Origin
  reconnects it, its connections numbered on; removed, it exits as such.
  `--url` and `--no-origin` go (one code path); a channel's Origin is in
  the list. Its status says what it listens to, as now.
- **Supervisor:** a `dgg-capture` per listed channel, as for Kick, each
  logging to `dgg-capture.<name>.log` (the name is what's between
  `dgg-capture.` and `.log`, dots and all), restarted on
  its own, the list read every minute to start the added. A dgg capture
  that hands off still stops the supervisor (an upgrade is under way); the
  other dgg channels' captures get the handoff wait Kick's get, then
  everything stops. A channel removed from the list ends its capture
  alone: `dgg-capture` exits with a code of its own for that (not 0), so
  the supervisor can tell the two apart. The old `dgg-capture.log` is kept
  and shown as a legacy log, as `kick-capture.log` is.
- **Install:** a bridge per dgg channel, as for Kick, each logging to its
  channel's log, all waited on together. Unlike Kick's, any dgg channel
  not healthy in time fails the install, as dgg's capture does now.
- **Staggered handovers:** an upgrade's handovers are spread out, not
  all at once, so dgg's and Kick's servers don't see every channel
  reconnect together (decided 2026-11-25). The install's bridges, and the
  supervisor's captures as it starts, are started one at a time, a few
  seconds apart (or each once the last is connected), so their
  connections, and the handoffs that follow each, are spaced the same
  way. Kick's history fills after a reconnect are spaced too. Exactly how
  is settled at step 4, and the install's wait grows to cover the spread.
- **Gaps and backfill:** `dgg-gaps --channel <name>` and `dgg-backfill
  --channel <name>`, each reading its channel's index and writing its
  backfill journal; `--channel` defaults to `destiny.gg` (not the list's
  first: they're given the data directory, not the install's root). The
  sources, rustlesearch (`Destinygg`) and linkers.ooo, hold destiny.gg's
  chat only: backfill refuses any other channel, saying no source has it.
- **Status:** a dgg capture names its channel, as a Kick one does
  (`Which` becomes service, channel and instance); `f` cycles dgg's
  channels' logs too; `r` goes round every channel's captures; the gaps
  page (`6`) shows the first channel's, and pressing again goes round the
  channels; the journal box's lines and the index's size are per channel.
- **The move,** by `migrate` at the supervisor's start, under the index,
  compact and sync locks, as the move by service did: everything in
  `dgg/` that's of the old layout (`live/`, `archive/`, `backfill/`,
  `control/`) into the list's first channel's directory,
  `dgg/destiny.gg/`, each a rename within the volume, a file already at
  its destination refused; and `index-dgg.sqlite` renamed to
  `index-dgg-destiny.gg.sqlite`.
  Safe to repeat.
  - An old capture's live files are left while it may still write them
    (its status fresh), moved by a later `migrate`; the old `control/` is
    removed once nothing in it is fresh. In an install nothing's left:
    the bridge starts before the old capture hands off, and `migrate`
    runs once the old supervisor has stopped, as last time. So nothing
    reads both places, unlike last time; a live file left behind waits
    for the next `migrate` to be indexed and compacted.
  - For the old capture to hand off, the first channel's capture (a
    bridge too) also heartbeats into the old `dgg/control/` while it's
    there, as last time.
  - Tested locally with both builds: the current one capturing, the new
    one installed over it; the handoff, what moved, the index against
    one rebuilt from scratch, and sync between two data directories on
    different builds (neither takes the other's files).
- **Steps,** each a pause for review:
  1. The list: parsing, the default, and `install` moving it. Nothing
     reads it yet.
  2. Routes by channel, and sync by channel, for both services. dgg has
     one channel, `dgg/` itself, until step 3. Rolled out to both hosts
     together, since builds on either side don't sync.
  3. dgg's layout by channel, its index per channel, gaps and backfill
     `--channel`, and the move. One capture still, the first channel's,
     with its old-control heartbeat.
  4. A `dgg-capture` per channel: `--channels` and `--channel`, the
     supervisor, the install's bridges, the logs.
  5. Status.
  6. Kick's index per channel: `serve`'s history route and `status`
     reading the channel's.
  7. Docs.
  8. Removing the old-control heartbeat and the move, once both hosts
     have moved.
- **The next channel:** `omniliberal.dev`, a dev chat speaking the same
  protocol as destiny.gg, added to the list once step 4 is in (its URL and
  Origin confirmed then). No backfill source has it, so its gaps stay
  gaps.

## Status
- **`logger-journal`:** live files, plus a writer and a reader. The `.zst`
  archives and manifests come with `compact`.
- **`logger capture`:** done, including local handoff.
- **`logger index`:** done for live files, one-shot or `--follow`. Following,
  it looks every `--interval` seconds (default 1) at how many records are
  waiting in the live and backfill files, counted without decoding them. A
  round is due once `--batch` (500) are waiting, but no sooner than
  `--min-wait` (60) seconds after the last, and at the latest `--max-wait`
  (600) seconds after it however few are waiting. It prints a summary every
  `--report` seconds, default 60.
  - Later, the newest messages, not yet indexed, could be searched directly
    (a small, recent set, scanned in full for each query), so the index can
    lag without searches missing anything.
- **`logger gaps`:** done. It reports stretches when no connection was open (a
  connection that never closed ends at its last record), and whether the next
  connection's `HISTORY` reached back far enough to cover each one.
- **`logger backfill`:** fetches uncovered gaps, or with `--from` a span of
  history, from rustlesearch or linkers.ooo into the journal's `backfill/`
  directory.
  - It writes one `fetched` record per message and a `fetchrange` per page.
  - It's limited by `--max-requests` (default 50) and stops when the rate limit
    has fewer than `--reserve` (10) requests left.
  - `--dry-run` lists the gaps it would fetch.
- **Backfill indexing:** done (schema 0.3).
  - Fetched messages are stored with `source = backfill:<source>` and matched on
    timestamp plus lowercased nick. Native rows win, whichever is indexed first.
  - A fetched nick takes its capitalisation from the same user's native
    messages.
  - `fetch_ranges` spans mark gaps as filled, and `backfill` skips filled gaps.
  - Live test: after backfill, the index holds exactly a reference capture's 583
    messages over the same span. 16 nicks stay lowercase because those users
    were never seen natively.
- **Kick:** steps 1 to 6 done: `kick-channels`, `kick-ids`, `kick-capture`,
  the supervisor running it, `status` showing it, compact and sync covering
  Kick's journals, Kick's index, and `serve`'s Kick endpoints (4 to 6 not
  yet run on a host).
  - The supervisor runs `serve`, given its section of the install's config,
    query token included (see [deploy](deploy.md#install-config)).
  - The supervisor finds the list in the install's root (its `--peers`,
    which every install passes), and runs a `kick-capture` per channel in
    it, reading it every minute (see
    [A capture per Kick channel](#a-capture-per-kick-channel)).
  - When dgg's capture hands off, `kick-capture` gets 90 s to hand off too
    before it's stopped.
  - `install` bridges Kick's capture as it does dgg's, with
    `kick-capture --bridge`, and waits for the old one to hand off to it.
    A Kick bridge that isn't healthy in time is only warned of: dgg's
    capture comes first. `kick-channels` moves with the root.
- **By service:** done: dgg and Kick each a service, dgg's journals in
  `journal/dgg/` (see [By service](#by-service)), on both hosts.
- **Cross-host handoff:** done. Tested live: the old instance retired about 38 s
  after the new one started.
- **`serve` / `sync`:** done. The supervisor's side is in [deploy](deploy.md).
- **Log levels:** done. Every line a program run by the supervisor logs
  starts with `INFO`, `WARN` or `ERROR` (`common::log`), the supervisor keeps
  it (a line without one is `INFO`), and `status --watch` colours them. Its
  first page is every log at once, in time order. A capture logs which
  older instance it took over from. `cert`, `kick-ids` and `gaps`, run by
  hand, print without levels.
- **`compact`:** done, and `index` reads archives (schema 0.4).
  - Archives are found through the monthly manifests.
  - Each archive's checkpoint carries its SHA-256, so a fully read archive is
    skipped unopened and a re-merged one is reread from the start.
  - Checkpoints of compacted-away live files are dropped.
  - Real data: the index rebuilt from the archive is identical to one built from
    the live files (535 messages).
