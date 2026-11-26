# dgg-tools planning

A Cargo workspace of tools around destiny.gg's chat, and Kick's. This
document holds the rules for every tool; each major tool's scope and
decisions live in its own document:
- [logger](logger.md) (codename ledgerline): the journal, capture, index,
  serve, sync, backfill, compact and gaps, for dgg and Kick.
- [deploy](deploy.md): the mesh of nodes running the logger.
- [dgg-api](dgg-api.md): destiny.gg's chat protocol and assets.
- [kick-api](kick-api.md): Kick's APIs.
- [kick-mod](kick-mod.md) (codename gavelgab): the Kick chat client and
  moderation tools.
- [viewer](viewer.md) (codename watcherweave): the read-only chat viewer.
- [archives](archives.md): archives and stats from chat and other dgg
  endpoints.
- [community](community.md): vouching, a pastebin and file uploads, a
  betting ledger, reputation and karma (notes only).
- [handoff](handoff.md): for a new session: how to work here, where things
  stand, and what's open.

Major components get lowercase codenames, by which they name themselves to
others, as in a `User-Agent`. The workspace's and the other components'
are still to be chosen.

## Layout
Each crate is a direct child of the root.
- `common/`: small tools any crate may share, a module each: `atomic_file`,
  `config` (JSONC configs and `Secret`), `format`, `fs`, `log`,
  `powershell`, `random`, `temp_dir` (for tests) and `time`.
- `dgg-api/`: destiny.gg's chat client, events and commands, and its chat
  assets.
- `kick-api/`: Kick's realtime client and events, its official API, the
  web API's chat history, and the channel lookup.
- `rustlesearch-api/`, `linkers-api/`: the APIs backfill reads.
- `logger-journal/`: the journal format, with a writer and a reader.
- `logger/`: the logger as a library, each command's arguments in
  `logger::cli`.
- `deploy/`: the `dgg-logger` binary (the logger's commands, plus install,
  uninstall, status, config and supervise) and `dgg-supervisor`, which runs
  the logger as a logon task or a Windows service. All one binary, so an
  install's parts can't be from different builds. See `deploy/README.md`.
- `kick-mod/`: the Kick moderation tool.
- `version-info/`: embeds version information in the Windows binaries.
- `viewer/`: the read-only chat viewer, a placeholder stub (formerly
  dggkick).
- Probes and one-off checks are examples: `dgg-api/examples/probe.rs`, say.

## Decisions
- **Stack:** Rust, with axum, tokio-tungstenite, reqwest and rustls (ring).
  Pages are plain JS with no build step (Node isn't installed here).
- **Style:**
  - rustfmt with `max_width = 80` and `tab_spaces = 2` (`rustfmt.toml`).
  - One `use { … }` block per scope, and a `::` prefix on every external
    crate, `std` included.
  - Tests go in each crate's `tests/` directory, never inline.
  - Test fixtures made from real data are anonymised before they're
    committed: users' names and ids, channels, and any other ids or uuids
    that trace back to a real account or session.
- **Shared tools:** a general-purpose tool goes in a module of `common`,
  even before a second crate needs it, rather than a crate of its own, so
  the workspace doesn't fill up with tiny crates. `common` depends only on
  small, canonical crates.
- **Deployment:** portable across Windows and Linux, and auditable, with
  nothing beyond the hosting itself: no VPN service, DNS provider or
  alerting service. See [deploy](deploy.md).
- **Versioning, for every format** (journal, `capture-status`,
  `journal-archive`, `journal-manifest`, the index schemas): like cargo's
  semver.
  - While the major is 0 (pre-release, where we are), any minor may break.
    Readers accept only the exact minor they know, and existing files are
    rewritten rather than read across versions.
  - Major 1 marks production. From then on, minor bumps may only add
    things old readers skip (new kinds, keys or entries); anything else is
    a major bump, which readers without that major's module refuse.
- **Config and secrets:** a command needing configuration beyond its
  flags, or any secret, reads it as JSON from `--config-file <path>` or
  `--config-stdin`, never from a flag's value or an environment variable.
  - Each command has its own type of what it reads, refusing fields it
    doesn't know and naming a missing one it needs.
  - Read as JSONC: `//` and `/* */` comments outside strings, and trailing
    commas. A hand-written one is named `.jsonc`, for editors
    (`kick-mod.env.jsonc`); one a program writes is plain JSON
    (`install.env.json`), as comments would be lost on the next write.
  - Secrets are `Secret`s, which never print: their `Debug`, and anything
    shown of the config, say `<secret>`. Each tool has a `config` command
    showing its config so.
  - The supervisor passes each command its part on stdin, so no secret is
    in an environment or on a command line.
- **Every outside service has its own API crate,** `<service>-api`, kept
  lightweight; tools don't talk to a service directly.
  - A crate holds where the service is, the requests, the answers as
    types (keeping fields they don't name, as kick-api does) and the
    service's limits, such as its rate-limit headers. Fetching is behind a
    feature; decoding needs none.
  - The `reqwest` client and its `User-Agent` are the caller's: each tool
    names itself by its codename and build, as
    `ledgerline/0.1.0+a3c579a14414`.
  - What a tool makes of the answers stays in the tool: backfill's
    matching and journaling, say.
- **Kick** has its own crates, apart from dgg's.

## Roadmap
1. **Logger:** done for dgg and Kick, running on two hosts. What's left is
   in [logger](logger.md#open) and [deploy](deploy.md#open).
2. **Kick moderation tools:** kick-mod's page parts, automod and a login
   ([kick-mod](kick-mod.md#open)).
3. **Read-only viewer:** see [viewer](viewer.md).
4. **Others, not yet ordered:**
   - a read-write chat client;
   - an embed logger, and embed transcriptions;
   - archival, and clipping tools;
   - a links and attachments viewer (like linkers.ooo);
   - an embed stats viewer: when embeds are live, how many viewers, and
     perhaps which;
   - a services dashboard: the captures and their health, `index` and
     `sync` progress, gaps, the backfill budget, built on `serve`'s
     channels' `control` and manifests.
5. **Archives and stats, not yet ordered:** see [archives](archives.md).
6. **Community services, not yet ordered:** see [community](community.md).
