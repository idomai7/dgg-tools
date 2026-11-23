# logger

The logger: `logger-journal` and `logger`, run as `dgg-logger` subcommands. See
the [overall rules](README.md), and [deploy](deploy.md) for the mesh it runs on.

## Decisions
- **Storage:** an append-only raw journal is the source of truth. SQLite
  (rusqlite, `bundled`) is an index that can be rebuilt from it.
- **Kick journals:** the journal format also covers Kick, for
  [kick-mod](kick-mod.md) now and a Kick logger later. It stays in
  `logger-journal`. Written by kick-mod; nothing compacts, syncs or indexes
  Kick's journals yet.
  - **Variants:** each platform is a variant of the format, versioned on its own
    as `<variant>.<major>.<minor>`, so one platform's changes never break the
    other's. Their events have little in common anyway.
    - Each variant's majors get their own modules in `logger-journal`, like
      `record/v0.rs` today: Kick's are under `record/kick/`.
    - Kick starts at `kick.0.0`. dgg's current `journal 0.1` files are dgg's,
      read as they are until dgg's logger moves to `dgg.0.<minor>`.
    - `kick.0.0` has dgg's record shape (`<receive-time> <instance>
      <connection> <kind> <data>`, shared code in `record/line.rs`), with only
      `frame`, `binary`, `open` and `close`. A frame is Centrifugo's raw text:
      one or more JSON replies separated by LFs.
    - **Framing:** each of `kick.0.0`'s records ends at RS LF (RS is 0x1E), so
      its data holds LFs verbatim and a record may span lines. Text data
      holding an RS is written base64, with the kind suffixed `64`, so an RS
      only ever ends a record, and a record with one inside is bad. Valid JSON
      can't hold an RS. The header ends at RS LF too, so every record, the
      header included, can be split on it.
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
    - dgg's header is still written without a variant or fields.
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
  - **dgg moves to RS LF:** dgg's journals move to Kick's framing,
    with records ending at RS LF, as `journal dgg.0.2`: dgg's variant named,
    its minor going on from 0.1.
    - A header without a variant stays dgg's LF-framed format. `journal 0.1`
      stays readable for good.
    - **Writers:** a day's file is appended to only if its header is the one
      the writer would write: the same version, and for Kick the same
      endpoint. Otherwise the day's next numbered file is started, so new
      records never land in an old file. (Capture's instances are new each
      run, but backfill's keep their names.)
    - **Compaction:** follows each input's framing; inputs may mix 0.1 and
      0.2, on the upgrade day or when a 0.1 archive is merged again.
      Archives are always written in the new version. A 0.1 record is copied
      byte for byte unless it holds an RS; then it's decoded and encoded
      again, as `frame64` and the like. A record that doesn't decode is kept
      byte for byte, as now; one that also holds an RS can't be, so the day
      is refused and reported.
    - **Archive manifests:** the monthly manifest is versioned like the
      journals, with a variant and informational fields:
      `journal-archive dgg.0.1`, and `journal-archive kick.0.0`. Its `lines`
      becomes `records`: what an archive holds, after its header, counted by
      its framing. `journal-archive 0.0` stays readable, and the migration
      rewrites it.
    - **Sync** counts an archive's records by its framing, to check them
      against its manifest.
    - **Sync's manifest** (`GET /manifest`) gets the same header, for the
      journal root it lists: `journal-manifest dgg.0.1`, and
      `journal-manifest kick.0.0` once Kick's journals are synced. Its
      entries hold what they did: live files by size, archives by size and
      SHA-256.
    - **The other line formats follow, for consistency:** after its header,
      ending at LF, each entry ends at RS LF, as the journal's records do.
      None of their values can hold an RS.
      - `journal-archive dgg.0.1` and `journal-manifest dgg.0.1`, not yet
        deployed, are defined that way.
      - `control/synced-archives` gains a header, `synced-archives dgg.0.1`;
        without one it's 0.0, a line each, still read and rewritten by the
        migration once the new one is checked to list the same.
      - `capture-status` in two phases, since an instance hands off only to
        one whose status it can read. This rollout reads `capture-status
        dgg.0.1` as well as 0.0, from files and from other hosts' pushes,
        and still writes 0.0. A later upgrade, once no instance reads only
        0.0, writes `dgg.0.1`.
    - **Rollout:** a logger without this can't read `dgg.0.2` journals or the
      new manifests, so every host is upgraded together. There are two.
    - **Migration:** existing `journal 0.1` journals are rewritten in the new
      format by a migration.
      1. It writes each file's new version alongside the old.
      2. It validates: every record of the new file is parsed and compared
         with the old file's, parsed by its own parser. Each must be
         byte-for-byte identical: same records, same order, same fields and
         data. A file that fails is reported, and nothing of it is removed.
      3. Only once the validation has passed are the old files removed.

      In detail (step C):
      - **What it covers:** the archives, both the journal's and the
        backfill journal's, and their monthly manifests. Live files aren't
        rewritten: compaction converts them into archives anyway, within a
        day or two, with the same checks. Rewriting them in place would also
        break the index, whose checkpoints for live files are offsets without
        a digest.
      - **Converting a record:** shared by the migration and compaction, in
        `logger-journal`. A record is copied byte for byte unless it holds an
        RS; that one is decoded and encoded again. Either way the result is
        decoded by the new parser and compared with the old parser's decoding
        of the original: the same record, or for one that doesn't decode the
        same error and the same bytes. Anything else fails.
      - **Converting an archive:** decompressed, converted record by record
        under a `journal dgg.0.2` header, compressed at compaction's default
        level, so both hosts turn the same archive into the same bytes.
      - **Validating it,** before anything is removed: the new file is
        written next to the old (`<day>.journal.zst.new`), fsynced, and read
        back. Both are read with `Reader`, each by its own framing, and
        compared record by record: as many, in the same order, each equal as
        above. Its size, bytes, records and SHA-256 are taken from the file
        as read back.
      - **Replacing it:** a month at a time. Once every archive of a month
        has been validated, the index is upgraded for them (below), then each
        is renamed over its old one, then the month's manifest is written
        (`journal-archive dgg.0.1`) with the new entries. A month with a
        failure is left as it was, and reported, and the rest carry on.
      - **Rerunning it** is safe: an archive already in `dgg.0.2` isn't
        converted again, only checked against its manifest entry, which is
        rewritten from the file if a crash came between the rename and the
        manifest. Leftover `.new` files are removed first. A conversion is
        deterministic, so one redone after a crash gives the same bytes and
        SHA-256 as the index was upgraded for.
      - `dgg-logger migrate --dry-run` converts and validates without
        replacing anything, and reports what it would do.
      - **Taken archives:** `control/synced-archives` lists peers' archives by
        their old SHA-256. Archives installed as they came convert to the
        same bytes on both hosts, so they still match. One that was merged
        with local files differs, so after both hosts migrate it's taken once
        more and merged again; deduplication makes that harmless.
      - **Upgrading the index,** rather than rebuilding it. Only two tables
        hold offsets into journal files: `files` (an archive's checkpoint and
        SHA-256) and `unparsed` (where a record that couldn't be indexed
        starts). Messages hold none. Validating an archive pairs each old
        record with its new one, so their offsets map exactly.
        - For each archive of the month, in one transaction: if its `files`
          row has the old SHA-256, its offset is mapped to the new file's
          (a record's end, or the header's), its digest set to the new
          SHA-256, and its `unparsed` rows' offsets mapped too (a record's
          start). An offset that isn't a record's boundary fails the month.
        - A row with any other digest, or none, is left alone: the index
          reads that archive again anyway, as it would have.
        - The transaction commits before the archives are renamed. A crash
          between leaves rows naming new SHA-256s while the old archives are
          still there; the index would read those again harmlessly, and the
          rerun converts them to exactly those SHA-256s.
        - Once every month has been migrated, the index's schema becomes
          0.5: the same tables, its archive offsets into `dgg.0.2` files. An
          index at 0.4 is refused, as now, so nothing indexes old offsets
          into new archives. `index --rebuild` still works at any time.
      - **In place, with nothing missed:** it runs during an ordinary
        upgrade, a new supervisor started alongside the old one, so capture
        never stops. There's one host, and no peers sync now.
        - The new supervisor starts its `capture` at once, as always: it
          only writes live files, already `dgg.0.2`. The old capture keeps
          going until the handoff, its last live files `journal 0.1`, which
          compaction converts later.
        - `migrate` holds `index.lock`, `compact.lock` and `sync.lock` while
          it runs, waiting for them like a supervisor does. So it starts
          only once the old supervisor has exited, and nothing else indexes,
          compacts or syncs meanwhile. Run by hand, it waits the same way.
        - The new supervisor runs `migrate` before taking those locks
          itself, and starts `index`, `compact` and `sync` once it has
          succeeded. If it fails, they don't, and the failure is in the
          supervisor's log and the status page; the next start tries again.
          Once every manifest is `dgg.0.1` and the index is at 0.5, `migrate`
          does nothing.
        - During the overlap, the old supervisor's programs meet the new
          capture's `dgg.0.2` live files: its index skips them as
          unreadable (the new index reads them from the start), and its
          compaction only touches closed days, which the new capture hasn't
          written.
        - Converting at level 19 is slow, so a large journal can take a
          while; capture runs meanwhile, and indexing catches up after.
      - **Rollout:** install the new build and start its supervisor, as for
        any upgrade. `migrate --dry-run` can be run by hand first; it only
        reads, so it takes no locks.
    - **Steps,** each its own commit and review. A and B ship together:
      without B, compaction splits the new files on LF.
      1. **A, logger-journal:** RS LF framing by header, `dgg.0.2`, the
         reader, the writers' crash repair and next-file rule, Kick's variant
         on the same framing.
      2. **B, logger:** compaction and sync following the framing, and the new
         archive and sync manifests.
         Both done; not deployed yet.
      3. **C, the migration:** `dgg-logger migrate`, and the supervisor
         running it. Done; not deployed yet.
      4. **D:** the rollout, on the one host: install the new build, as
         for any upgrade, and watch `migrate.log`. Done on one host, which
         is at `dgg.0.2`; the other is still at `journal 0.1`.
    - **Headers end at RS LF too:** so every record, the header included,
      can be split on RS LF. The header still ends at the first LF, with the
      RS before it, so a reader finds it before it knows the framing. Each
      format says from which version its header ends at RS LF; older
      versions are still read.
      - New versions: `journal dgg.0.3`, `journal-archive dgg.0.2`,
        `journal-manifest dgg.0.2` and `synced-archives dgg.0.2`. `kick.0.0`
        (journal and manifest), never written, is redefined.
      - `capture-status dgg.0.2` is read, but not yet written: phase 1
        again, as for `dgg.0.1`, which was never written either.
      - `migrate` takes archives of `journal 0.1` or `dgg.0.2` to
        `dgg.0.3`: from 0.2, only the header changes, every offset moving
        a byte on. The index goes from schema 0.4 or 0.5 to 0.6.
      - **Rollout:** first the host at `dgg.0.2`, then the other, which
        goes from `journal 0.1` to `dgg.0.3` at once. Both paths were run
        with the real builds (the original, the deployed one, and this),
        each checked against an index rebuilt from scratch.
      - Found along the way, as every build has done: the index keeps
        `unparsed` rows for live files compaction has deleted, as
        `forget_gone` only forgets their `files` rows.

## Design
- **Journal layout:**
  - `data/journal/live/YYYY-MM-DD/<instance>.journal`: one file per capture
    instance per day. These are the hot files.
  - `data/journal/archive/YYYY/MM/YYYY-MM-DD.journal.zst` plus a `manifest`
    per month (`journal-archive dgg.0.2`). These are cold.
  - `data/journal/control/<instance>.status`: heartbeats (`capture-status 0.0`;
    see `logger/src/control.rs`).
  - `data/journal/backfill/YYYY-MM-DD/<instance>.journal`: what was fetched from
    other archives, one file per backfill run per day. It's filed by the day of
    the messages inside, not when they were fetched. Nothing here is canonical,
    so it never mixes with `live/` or `archive/`.
  - Hot paths only ever list `live/`.
- **Days:** UTC.
- **Record format:** explicit and hand-written, with no serde. The spec lives in
  the `logger-journal` crate: each major version has its own module
  (`logger-journal/src/record/v0.rs`) holding its structure, encoding and
  decoding, so several formats can coexist in the library.
  - Each file starts with `journal dgg.<major>.<minor>` (currently `0.3`;
    `journal 0.1` before variants), ending at RS LF (at LF up to 0.2).
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
  - **Across hosts:** the old host runs `serve --control-token …`, and the new
    instance runs
    `capture --peer http://old-host:8788 --peer-token <that control token>`.
    - `serve`'s plain `--token` only reads (enough for `sync`); pushing a status
      needs the control token, since a pushed status makes the host's captures
      retire.
    - The new instance pushes its status there every second
      (`PUT /control/<instance>`), shifted onto the old host's clock, and
      withdraws it on exit.
    - The old instance retires by the same rule as a local handoff, and needs no
      flags or restart.
    - Use a VPN or SSH tunnel between hosts: the bearer token isn't encrypted
      over plain HTTP.
- **index:** journal into `data/index.sqlite` (schema in
  `logger/schemas/index/v<major>.<minor>.sql`, currently 0.4, every past version
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
  - `--rebuild` replays the whole journal. An index from another schema version
    must be rebuilt.
- **serve / sync:** `serve` also lists live files (`GET /manifest`, format
  `journal-manifest dgg.0.2`) and serves their bytes
  (`GET /files/live/<day>/<instance>.journal`, honouring `Range: bytes=N-`).
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
      logged-in `lk_session` cookie (`--linkers-session` / `LINKERS_SESSION`,
      valid 7 days) removes the limit.
    - Also logs `BAN`, `MUTE`, `SUBSCRIPTION`, `GIFTSUB`, `MASSGIFT` and
      `DONATION`, which could feed the archive tools.
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

## Status
- **`logger-journal`:** live files, plus a writer and a reader. The `.zst`
  archives and manifests come with `compact`.
- **`logger capture`:** done, including local handoff.
- **`logger index`:** done for live files, one-shot or `--follow` (polls every
  `--interval` seconds, default 1; prints a summary every `--report` seconds,
  default 60).
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
- **Cross-host handoff:** done. Tested live: the old instance retired about 38 s
  after the new one started.
- **`serve` / `sync`:** done. The supervisor's side is in [deploy](deploy.md).
- **`compact`:** done, and `index` reads archives (schema 0.4).
  - Archives are found through the monthly manifests.
  - Each archive's checkpoint carries its SHA-256, so a fully read archive is
    skipped unopened and a re-merged one is reread from the start.
  - Checkpoints of compacted-away live files are dropped.
  - Real data: the index rebuilt from the archive is identical to one built from
    the live files (535 messages).
