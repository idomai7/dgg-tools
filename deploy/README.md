# Running the logger

The logger is one program, `dgg-logger`, whose commands share one data
directory:

| Command | Job | Runs |
|---|---|---|
| `capture` | Connects to dgg chat and writes everything to the journal | all the time |
| `index` | Reads the journal into `index.sqlite` | all the time, with `--follow`: once 500 records are waiting (at most once a minute), or every 10 minutes |
| `compact` | Packs each finished day into one compressed archive | every hour |
| `gaps` | Lists when no capture was connected | when you want |
| `backfill` | Fetches missed messages from public logs | after an outage |
| `sync`, `serve` | Copy the journal between hosts | when moving hosts, or every 15 minutes from a peer |
| `kick-ids` | Records Kick channels' ids in `kick-channels`, for `kick-capture` | when adding a channel |
| `kick-capture` | Connects to Kick chat, each channel in `kick-channels`, and writes everything to the journal | all the time, once `kick-channels` has channels |
| `install`, `uninstall`, `status`, `supervise` | Run, install and inspect the rest | when you want |

`dgg-supervisor` keeps capture and index running, restarts them after
failures, and runs compact (and sync, given a peer); it's `dgg-logger
supervise` without a window, for the logon task and the service. It runs
`kick-capture` too once `kick-channels` has channels (it looks every
minute until then). A change to the list takes effect when the supervisor
next starts. In Task Manager, both show as "dgg-tools: ..."; the Details tab's Command line
column tells the commands apart.

The commands that work on the data (`capture` to `serve`) take `--data`,
which defaults to the installed logger's.

An install lives in one folder:

```
%LOCALAPPDATA%\dgg-tools\        (C:\ProgramData\dgg-tools for a machine install)
  bin\<time>-<version>+<commit>\  one build per install; older ones are pruned
  data\
    journal\
      live\                today's raw capture, one file per capture per day
      archive\             finished days, compressed
      backfill\            messages fetched from other logs, kept apart
      control\             each running capture's status
      kick\<slug>\         each Kick channel's journal, laid out the same
    index.sqlite           the searchable index; can be rebuilt from the journal
  logs\                    capture.log, index.log, compact.log, supervisor.log,
                           sync.log, migrate.log, kick-capture.log
  nodes, mesh.env          the mesh this node is in, if any (mesh.env holds
                           its token)
  peers\                   other hosts to sync from (each holds a token)
  kick-channels            the Kick channels to capture, from `kick-ids`
```

The journal is the record. The index can always be deleted and rebuilt with
`dgg-logger index --rebuild`. Logs are timestamped and rotated at 10 MB, keeping 5.

## Build

You need Rust (`cargo`). From the repository:

```powershell
cargo build --release
```

Everything lands in `target\release\`. The commands below run from there;
`install` copies the build it's run from, so the build folder stays free
for the next `cargo build`.

## Install

```powershell
target\release\dgg-logger install
```

This installs for your account: a scheduled task, `dgg-logger`, starts the
supervisor at logon, with no window. It needs no administrator rights. The
install waits until its capture is connected and healthy, then says so.

Or, from an administrator prompt, for the machine:

```powershell
target\release\dgg-logger install --machine
```

That's a Windows service, `dgg-logger` ("dgg-tools logger" in
services.msc), started with the system whether or not anyone is logged on,
and restarted a minute after any failure. It runs as its own account,
`NT SERVICE\dgg-logger`, not SYSTEM, and that account may change only the
data and logs. The folder is writable only by administrators, since anyone
who could change it could run code as the service; other users can read
it. Running `backfill`, `gaps` or `index` by hand against a machine
install's data needs an administrator prompt.

`--root`, `--data` and `--logs` put things elsewhere.

### Upgrading and switching

Build, then run `install` again, the same way. Nothing is missed: a capture
from the new build starts first (the "bridge", its lines in `capture.log`
marked `(bridge)`),
the old install hands over to it and stops, and then the new install starts
and takes over from the bridge. Old builds are pruned, keeping three.

With Kick channels, Kick's capture is bridged the same way, its bridge's
lines in `kick-capture.log`. If that bridge isn't healthy in time,
`install` warns that Kick may miss a few seconds, and carries on: dgg's
capture comes first. `kick-channels` moves with the install's folder.

Running `install` with the other kind (with or without `--machine`)
switches to it the same way, moving the data and logs to the new folder.
Anything involving a machine install needs an administrator prompt.

If anything goes wrong, `install` says what, and the capture that was
running (or the bridge) keeps running.

## Status

```powershell
target\release\dgg-logger status
target\release\dgg-logger status --watch
```

It finds the installed logger and shows:

- the running programs: memory, CPU, and which build;
- each capture's health (healthy, connecting, or STALE once its heartbeat
  stops), and the frames in its files not yet compacted;
- the journal: today's files and their last write, days waiting for
  compact, archives, backfill;
- the index, free disk space, the peer it syncs from (and how the last
  round went), and the ends of the logs;
- Kick's captures and their health, and each channel's journal today, if
  Kick is captured.

`--watch` keeps it open, redrawing every second (`--interval` to change)
with how fast today's journal is growing and a page filling the rest of the
window. The keys are grouped:

- `l` every log at once, in time order, each line naming its log (the page
  it starts on); `r` the latest records of the newest capture (`r` again
  for the next capture); Space redraws, `e` opens the data folder, `q`
  quits;
- each program's log: `1` the supervisor's, `2` migrate's, `3` capture's,
  `4` compact's, `5` index's, `6` the gaps in capture as `gaps` reports
  them (looked at again every 30 seconds; it reads the index as `index`
  has brought it, without updating it), `7` sync's;
- `8` kick-capture's log.

In a narrow window the keys take two lines.

Each log line has its time and a level, `INFO`, `WARN` (something's wrong,
but work goes on) or `ERROR` (something failed or stopped):

```text
2026-11-24 09:41:20.512Z WARN  waiting for compact.lock: another supervisor has it
```

The programs say their own lines' levels, and the supervisor keeps them; a
line that doesn't say is `INFO`. `--watch` dims the times, and draws warnings
in yellow and errors in red.

## Running it in a shell instead

Without installing anything:

```powershell
target\release\dgg-logger supervise --data "$env:LOCALAPPDATA\dgg-tools\data" --logs "$env:LOCALAPPDATA\dgg-tools\logs"
```

That's the same supervisor, in the console, until Ctrl-C. Using the user
install's folders means a later `install` picks the data up where it is.
While it runs, `cargo build` can't replace the programs it's running; copy
`target\release` somewhere first to keep building. Installing later takes
over from it without a gap.

## After an outage

If the PC was off or asleep, see what was missed and fill it from public
logs:

```powershell
target\release\dgg-logger gaps --min 60
target\release\dgg-logger backfill --source linkers --dry-run
target\release\dgg-logger backfill --source linkers
```

Short gaps are often already covered by the chat history dgg sends on every
reconnect; `gaps` says which ones were. Backfilled messages go in
`journal\backfill\`, separate from what was captured, and never replace a
captured message. `--source rustlesearch` is millisecond-accurate but
limited to 200 requests a day.

Don't start a history import (`backfill --from`) by hand yet: it's
meant to run for weeks and comes later.

## Syncing from other hosts

An install can also pull other hosts' journals every 15 minutes, so it
has whatever they captured while this one was off. Each other host runs
`dgg-logger serve` with a token (see below). Peers are synced from one
after another; one that fails doesn't stop the rest.

### A mesh of nodes

When several hosts all capture, give every one the same node list: one
line per node, its name and where its `serve` is, or `push` for a node
that can't be reached (behind NAT):

```
# name    where
do        https://203.0.113.5:8788
oracle    https://198.51.100.7:8788
pi        push
```

Then, on each node, with the mesh's read token (the same on every node):

```powershell
$env:LOGGER_MESH_TOKEN = "<the mesh token>"   # or LOGGER_MESH_TOKEN=... in .env
target\release\dgg-logger install --name pi --nodes nodes
```

Each node syncs from every other node with a URL. The list is copied to
`nodes` in the install's folder, and `--name` and the token go in
`mesh.env`; to change the list, edit it and run `install --nodes` again on
each node (or edit `nodes` by hand: it's read before each round).
`install --no-mesh` leaves the mesh.

### One-off peers

To also sync from a host outside the mesh, such as one being moved off:

```powershell
$env:LOGGER_PEER_TOKEN = "<its token>"   # or LOGGER_PEER_TOKEN=... in .env
target\release\dgg-logger install --peer http://other-host:8788
```

That writes `peers\other-host.env` (`--peer-name` names it otherwise).
`install --no-peer other-host` removes it, and `--no-peers` all of them.
An older install's `peer.env` is moved there by the next `install`.

Every file is read before each round, so they can be edited by hand too.
In a machine install, only administrators and the service can read the
ones holding tokens. The next `install` keeps them; `uninstall` removes
them. Each round's output is in `sync.log`, each line after the peer's
name; `status` shows each peer's last round. The index picks up what
arrives.

### Running serve safely

On each host synced from, make a token and serve with it:

```powershell
$env:LOGGER_TOKEN = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32))
target\release\dgg-logger serve --listen 0.0.0.0:8788
```

- `serve` speaks plain HTTP, so the token crosses the network readable.
  Reach it only over something private: a VPN such as Tailscale or
  WireGuard, or an SSH tunnel. Don't open the port to the internet.
- That token only reads, which is all sync needs. A second one,
  `--control-token` (`LOGGER_CONTROL_TOKEN`), lets a capture on another
  host push its status there, which makes that host's captures retire.
  Set it only while moving capture to a new host (`capture --peer`), and
  give it only to that host. Tokens must be 32 characters or more, and
  the two must differ.
- Sync trusts the peer for what its journal says: whatever it serves is
  added here. Sync from your own hosts only. It doesn't trust the peer
  for size, though: it refuses a manifest over 16 MB and a file over 4 GB,
  takes no more of a file than the manifest lists, checks archives
  against their SHA-256, and doesn't follow redirects.
- `serve` handles 8 requests at once and turns away the rest.

## Keeping the PC awake

A user install only runs while you're logged on, and nothing runs while the
PC sleeps. While plugged in:

```powershell
powercfg /change standby-timeout-ac 0
```

## Uninstalling

```powershell
target\release\dgg-logger uninstall
```

It stops everything running from the install's `bin\`, removes the task or
service, and deletes the binaries and the peers' files. Only the data and logs
are left, and the
folder holding them; a machine install's get ordinary permissions back,
owned by you, so you can move or delete them without elevation. A machine
install needs an administrator prompt to remove. Delete the data and logs
by hand to remove everything.
