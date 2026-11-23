# dgg-tools planning

A Cargo workspace of tools built around destiny.gg chat and its endpoints. Every
tool that talks to dgg goes through the shared `dgg-api` crate.

This document holds the rules for every tool. Each major tool's scope and
decisions live in its own document here:
- [dgg-api](dgg-api.md): destiny.gg's chat protocol and assets.
- [logger](logger.md) (codename ledgerline): the journal, capture, index,
  serve, sync, backfill, compact and gaps.
- [deploy](deploy.md): the mesh of nodes running the logger.
- [viewer](viewer.md) (codename watcherweave): the read-only chat viewer.
- [archives](archives.md): archives and stats derived from chat and other dgg
  endpoints.
- [kick-api](kick-api.md): Kick's APIs.
- [kick-mod](kick-mod.md) (codename gavelgab): the Kick chat client and
  moderation tools.

Major components get codenames, by which they name themselves to others, as
in a `User-Agent`: so far the logger's, ledgerline, kick-mod's, gavelgab, and
the viewer's, watcherweave. Codenames are always lowercase. The workspace's and
the other components' are still to be chosen.

## Layout
The layout is flat: each crate is a direct child of the root.
- `common/`: small tools any crate may share, one module each: `atomic_file`,
  `format`, `fs`, `powershell`, `random`, `temp_dir` (for tests) and `time`. It
  depends on little beyond `std`: getrandom, base64 and fastrand, for
  `random`.
- `dgg-api/`: destiny.gg's APIs, kept lightweight for any tool to depend on: the
  chat socket client and event types, and more of dgg's endpoints to come.
- `kick-api/`: Kick's APIs, kept lightweight for any tool to depend on. So far,
  decoding the unofficial channel lookup, and decoding realtime frames into
  Centrifugo's client protocol.
- `rustlesearch-api/`: rustlesearch.dev's API, which backfill reads: decoding
  its searches and rate limit, and with feature `http`, fetching them.
- `linkers-api/`: linkers.ooo's API, which backfill reads: decoding its pages
  of every kind of message and its rate limit, and with feature `http`,
  fetching them.
- `kick-mod/`: a chat client and moderation tools for Kick channels. So far,
  `kick-mod ids`, which looks up channels' ids with curl.
- `logger-journal/`: the logger's journal file format, with a writer and a
  reader.
- `logger/`: the logger as a library: capture, index, serve, sync, backfill,
  compact and gaps, each with its command-line arguments in `logger::cli`. They
  run as `dgg-logger` subcommands, so the parts of an install can't be from
  different builds.
- `deploy/`: `dgg-logger` (the logger's commands, plus install, uninstall,
  status and supervise) and `dgg-supervisor`, which keeps the logger running as
  a logon task or a Windows service. See `deploy/README.md`.
- `version-info/`: a build helper embedding version information in the Windows
  binaries.
- `viewer/`: the read-only chat viewer, formerly dggkick.

## Decisions
- **Stack:** Rust. Node isn't installed on this machine.
  - axum, tokio-tungstenite, reqwest, rustls (ring).
  - Pages are plain JS with no build step.
- **Style:**
  - rustfmt with `max_width = 80` and `tab_spaces = 2` (`rustfmt.toml`).
  - One `use { … }` block per scope, and a `::` prefix on every external crate,
    `std` included.
  - Tests go in each crate's `tests/` directory, never inline in the source.
  - Test fixtures made from real data are anonymised before they're committed:
    users' names and ids, channels, and any other ids or uuids that trace back
    to a real account or session.
- **Shared tools:** a general-purpose tool goes in a module of `common`, even
  before a second crate needs it, rather than in a crate of its own, so the
  workspace doesn't fill up with tiny crates. Its modules depend on little
  beyond `std`, and only on small, canonical crates.
- **Deployment:** must stay portable across Windows and Linux.
  - Auditable, with nothing beyond the hosting itself: no VPN service, DNS
    provider or alerting service.
  - See [deploy](deploy.md).
- **Versioning, for every format** (journal, `capture-status`,
  `journal-archive`, `journal-manifest` and the index schema): like cargo's
  semver.
  - While the major is 0 (pre-release, where we are now), any minor may break.
    Readers accept only the exact minor they know, and existing files are
    rewritten rather than read across versions.
  - Major 1 marks production. From then on, minor bumps may only add things old
    readers skip (new kinds, keys or entries), and anything else is a major
    bump, which readers without that major's module refuse.
  - Everything was re-versioned from 1.x to 0.x on 2026-11-09, and existing
    files were rewritten to match.
- **Kick:** its own crates, separate from dgg's: [kick-api](kick-api.md) and
  [kick-mod](kick-mod.md).
- **Every outside service has its own API crate,** `<service>-api`, as dgg's and
  Kick's do, kept lightweight for any tool to depend on. Tools don't talk to a
  service directly.
  - A crate holds where the service is, the requests, the answers as types
    (fields they don't name kept, as kick-api does) and the service's limits,
    such as reading its rate-limit headers. Fetching is behind a feature, and
    decoding needs none.
  - The `reqwest` client and its `User-Agent` are the caller's: each tool
    names itself by its codename, as `ledgerline/<version>`.
  - What a tool makes of the answers stays in the tool: backfill's matching
    and journaling, say.

## Roadmap
1. **Logger** (current focus): see [logger](logger.md).
2. **Read-only viewer:** see [viewer](viewer.md).
3. **Others, not yet ordered:**
   - read-write chat client;
   - embed logger;
   - embed transcriptions;
   - archival;
   - clipping tools;
   - links/attachments viewer (like linkers.ooo);
   - embed stats viewer: when embeds are live, how many viewers, and perhaps
     which viewers;
   - services dashboard: the status of running services, such as capture
     instances and their health, `index --follow` and `sync --follow` progress,
     gaps, and the backfill budget. It could build on `serve`'s `/control` and
     `/manifest`.
4. **Archives and stats, not yet ordered:** see [archives](archives.md).
5. **Kick moderation tools** (in planning): see [kick-mod](kick-mod.md), and
   the logger's Kick capture, which it builds on (see [logger](logger.md#kick)).

## Status
- **Workspace restructure:** done.
  - The dgg probe is at `dgg-api/examples/probe.rs`.
  - `viewer` is a placeholder stub.
- Each tool's status is in its own document.
- **Next:** pick from the roadmap: the read-only viewer, the archives and stats
  tools, or the services dashboard.
