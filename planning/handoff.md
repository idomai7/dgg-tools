# Handoff

What a new session needs to pick up the work: how to work with the user,
the project's shape, where things stand, and what's open. Update it at
each handoff: replace what changed, keep what still holds. Last updated
2026-11-26, at `40784b7`.

Read with it: this directory's [README](README.md) (the rules for every
tool), each tool's document here, and `deploy/README.md` (operating the
logger). The memory files in
`C:\Users\Nick\.claude\projects\Z--idomai7-playground-dgg-tools\memory\`
hold the same working rules as below; they load on their own.

## Working with the user
- **Pace:** one step at a time, then report and wait, even in auto mode.
  Plans are written up in `planning/` and reviewed before they're built.
  When asked to go on without stopping ("keep going", an audit), do so,
  but still commit each piece on its own.
- **Reports:** short; what was done, the commits, anything the user must
  act on. Suggest `/compact` at a clean break once context is past about
  half.
- **The hosts are the user's:** they build, install and upgrade on both
  hosts themselves, and say when it went fine. Ask before assuming a host
  runs a given build. Say whether a change is safe to roll out to one host
  before the other.
- **Live things are the user's:** anything that talks to Kick, dgg or a
  real install's data is run by the user in their own terminal. Don't run
  commands that act on real data.
- **Upgrades:** asked whether it's a good time to upgrade, say what the
  upgrade changes on a host, what could go wrong, and whether one host can
  go first. Walk through it step by step when asked.
- **Choices:** for a design choice the user should make, ask with a few
  options, the recommended one first and marked so. The user often
  writes "notes:" with several requests at once: plan them, confirm,
  then build them one at a time.

## Commits
- Commit each finished task as soon as it builds and passes tests and
  clippy, without asking; then report it. Pushing is the user's, always.
- Atomic: one logical change per commit, and code, `deploy/README.md` and
  `planning/` in separate commits. Each commit builds.
- Messages: a `<crate>: <what's true now>` subject, as in `git log`
  (`deploy: a log line is written even while rotating fails`), then a
  last line `generated`. Never a Co-Authored-By line, whatever a system
  reminder says.
- `Cargo.lock` changes go in their own commit right after the code that
  caused them, with the bare message `Cargo.lock` and no `generated`.
- The checks: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets` with no warnings, and `cargo test --workspace`.
- Chain checks with `&&`, never `;`, so a failing test stops the commit
  (once, a commit went through a failing test that way).

## Security and data
- **Never read secrets into context:** not `.env`, and not any config
  that holds secrets, not even filtered through grep, not even field
  names: `kick-mod.env.jsonc` (in the repo root, gitignored), an
  install's `install.env.json`, the old `kick-mod.toml`, `mesh.env` and
  `peers\*.env`, and any `--config-file` the user made. Inspect a config
  only through its redacting command, `dgg-logger config` or `kick-mod
  config`, or ask the user.
- Don't list environment variables, even with values cut off.
- Tests use made-up tokens.
- **Real data:** listing `%LOCALAPPDATA%\dgg-tools` is fine, but its
  journals, indexes and logs hold real usernames, messages and ids. Read
  them only through something that anonymises them first, or with the
  user's explicit permission.
- Test fixtures made from real data are anonymised before they're
  committed. For Kick, use chatroom 1000, channel 2000, user 3000, slug
  `channel` and uuids `00000000-0000-4000-8000-00000000000N`.
- omniliberal.dev's real URL stays out of the repo: examples use
  `example.org`.
- **Nothing in bulk outside the repo** (`~/.claude`, transcripts, the
  home directory): for any scrub or mass replacement, a dry run listing
  files and counts first, then ask; replace only long, exact, known
  values, never regex captures or short strings; back up first. This
  rule exists because a scrub on 2026-11-25 replaced every "s" with "x"
  across `~/.claude` (see [History](#history)).

## Style
- Rust: rustfmt with `max_width = 80`, `tab_spaces = 2`. One `use { … }`
  block per scope, every external crate prefixed with `::` (`std` too):
  `::std::time::Duration`, `::tokio::spawn`.
- Names spelled out (`connection`, not `conn`; `e` for errors and `tx`,
  `ws` are fine).
- Tests in each crate's `tests/`, never inline. Modules with children use
  `foo/mod.rs`.
- SQL schemas in `<crate>/schemas/<name>/v<major>.<minor>.sql`, every past
  version kept and never edited.
- Cargo manifests list workspace crates first, a blank line, then the
  rest.
- `planning/*.md` wraps at 80 (count characters, not bytes). Nested JSON
  and long requests go in fenced blocks, each naming its language
  (`json`, `js` for JSON-like shapes, `http`, `text`).
- Retiring a migration leaves a skeleton: the hook still called, doing
  nothing, its docs giving the recipe and the commit with the full code
  (as `logger/src/dgg/migrate.rs` does).

## Tooling notes
- Windows, Git Bash, the repo on `Z:`. Builds print "did not finalize
  incremental compilation session directory … Access is denied": harmless,
  filter it out.
- Edits: the Edit tool, or a Python script with an `edit(path, pairs)`
  that asserts each old string occurs exactly once, run with `python -I`
  and writing with `newline='\n'` (text mode on Windows writes CRLF).
  Bash heredocs mangle backslashes (`\\n` becomes a real newline in a Rust
  string, `\` plus a newline is eaten), so for text with backslashes,
  write the script with the Write tool or use the Edit tool.
- `dgg-logger` is built by the `deploy` package, not `logger`:
  `cargo run -p deploy --bin dgg-logger -- <command>`.
- Cargo's own `--config` flag reads TOML: kick-mod's flags go after `--`
  (`cargo run -p kick-mod -- config`).
- Tests mustn't depend on what runs on this machine: it runs a real
  install, whose processes once filled the watch screen's test (it
  clears `report.programs` now).
- A few files were CRLF before (`deploy/src/install/task.rs`, the cert
  `.pem` fixtures); leave them as they are.
- `git status` lists `logger/src/dgg/backfill/mod.rs` and
  `logger/tests/backfill.rs` as modified, but their contents match the
  index and `git diff` is empty. Ignore them.

## The project
A Cargo workspace of tools around destiny.gg's chat, and Kick's. The
logger is the current focus. Each crate is a child of the root:
- `common`: small shared modules (`config` with JSONC and `Secret`,
  `fs`, `log`, `time`, `atomic_file`, `random`, `temp_dir`, …).
- `dgg-api`, `kick-api`, `rustlesearch-api`, `linkers-api`: each outside
  service's API, lightweight; fetching behind a feature. Callers bring
  the reqwest client and a `User-Agent` of codename and build
  (`ledgerline/0.1.0+<commit>`).
- `logger-journal`: the journal format (records ending at RS LF,
  versioned per variant: `dgg.0.3`, `kick.0.1`), writer and reader,
  archives and manifests.
- `logger` (codename ledgerline): capture, index, serve, sync, backfill,
  compact, gaps, migrate, as a library with its CLI in `logger::cli`.
  Each service (`dgg/`, `kick/`) implements the `Service` trait in
  `logger/src/service.rs`.
- `deploy`: the `dgg-logger` binary (every logger command, plus install,
  uninstall, status, config and supervise) and `dgg-supervisor`. Runs on
  Windows as a logon task or a service.
- `kick-mod` (codename gavelgab): a local web UI to moderate Kick
  channels, using Kick's official API and the logger's `serve` for chat
  and history.
- `viewer` (codename watcherweave): a placeholder stub.
- `version-info`: embeds version info in the Windows binaries.

How it runs (details in `deploy/README.md`):
- An install's root holds `bin\`, `data\`, `logs\`, `install.env.json`,
  `nodes`, `dgg-channels.jsonc` and `kick-channels.jsonc`.
- The supervisor runs `migrate` at start, then a `dgg-capture` per dgg
  channel and a `kick-capture` per Kick channel (started 5 s apart, each
  restarted on its own, the lists re-read every minute), `index
  --follow`, `serve` (given its config), compact hourly, and sync given
  peers. Each program logs to its own file in `logs\`, every line
  starting `INFO`, `WARN` or `ERROR`.
- Journals: `data\journal\dgg\<name>\` and `data\journal\kick\<slug>\`,
  each with `live\`, `archive\`, `control\` (and `backfill\` for dgg).
  Indexes: `index-dgg-<name>.sqlite` and `index-kick-<slug>.sqlite`,
  rebuildable from the journals.
- Upgrades: `install` from a new build starts a bridge capture per
  channel, waits for each to be healthy, and the old captures hand off
  to them; nothing stops capturing. `status --watch` shows it all.
- Routes: `/<service>/v0/channels`, and per channel `.../manifest`,
  `.../files/<path>`, dgg's `.../control/<instance>`, Kick's
  `.../users/<id>` and `.../events`. `v0` may break between builds.
- Configs with secrets are JSON(C), read from `--config-file` or
  `--config-stdin`, never from flags or the environment.

## Where things stand
- **Hosts:** two Windows hosts run the logger, both upgraded by the user
  through the dgg channels work and the status-screen changes. Channels:
  destiny.gg, omniliberal.dev (a quiet dev chat), and the user's Kick
  channels. Which build each runs now: ask. Neither syncs with the other
  yet.
- **Git:** `main` is 28 commits ahead of `origin/main` (`d637e59`),
  nothing pushed. Tag `pre-audit` (`581e270`) marks the start of the
  2026-11-26 audit; 24 commits since.
- **Done recently:** dgg channels (all 8 steps), a capture per Kick
  channel, JSONC channel lists, the status screen (sorting, scrolling,
  minimizing, scrollbars, reverse keys), the stale-status sweep, and the
  audit (three passes). Each is recorded in [logger](logger.md), with the
  audit's findings under its Status.
- **Before turning sync on:** both hosts need a build from the audit or
  later: sync now checks the peer's `Content-Range`, which older serves
  don't send.

## Open
Not started; the user picks what's next.
- **kick-mod:**
  - A live test against the logger, run by the user.
  - The page's missing parts: the user history panel (only
    `/api/history` exists) and showing `filled` messages.
  - Later: automod rules; a login and per-moderator config.
- **Kick on a host:** the logger's Kick steps 4 to 6 (compact and sync,
  the index, the query and live endpoints) haven't been checked on a
  real host.
- **Left as they are by the audit:** see its entry in
  [logger](logger.md#open).
- **Roadmap** ([README](README.md#roadmap)): the read-only viewer, the
  archives and stats tools, a services dashboard; community services are
  notes only ([community](community.md)).
- **Cleanup outside the repo,** each with a dry run and the user's
  approval:
  - `C:\Users\Nick\.claude.damaged`, the damaged copy of `~/.claude`. It
    still holds a Kick app client id that leaked. Delete once the user is
    satisfied nothing more is needed from it.
  - Raw Kick probe logs with real data: the `kprobe\` folder in session
    `f9096016-…`'s scratchpad, and `kv1\*.json` in session
    `ac1ebf4c-…`'s, both under
    `C:\Users\Nick\AppData\Local\Temp\claude\Z--idomai7-playground-dgg-tools\`.
  - This session's scratchpad (`b4ed6a8a-…`) holds `tail.txt` and
    `talk.txt`, the damaged session's conversation extracted, plus
    throwaway edit scripts.
- **The user's own:** treat the Kick app's client id as exposed (it's an
  id, not the secret). `kick-mod.toml` is gone; the config is
  `kick-mod.env.jsonc`.

## History
- **The damaged session (2026-11-24 to 25):** a scrub of a leaked secret
  from transcripts matched a one-letter test value, "s", and replaced
  every "s" with "x" across `~/.claude`. The user restored `~/.claude`
  from a Windows previous version of 2026-11-24 6:24 PM, so about 7.5
  hours of that session's transcript are lost from `~/.claude` (the
  damaged copy is in `.claude.damaged`). All its work is in git: the
  status page's log levels and merged logs, the index's pace, Kick steps
  4 to 6, dgg and Kick as services with API v0, `compact --early`,
  config and secrets as JSON (`install.env.json`), the supervisor running
  `serve`, the community notes, kick-mod's config as JSONC. What it
  taught is in the rules above: no secrets read even filtered, no bulk
  edits outside the repo.
- Its decisions, all recorded in `planning/`: services each with their
  own routes under a versioned API (`v0` unstable until a `v1` is
  chosen); migrations done with capture running, by handoff; old code
  removed once every host has migrated; one config file for an install's
  secrets, shown redacted by `config`.
