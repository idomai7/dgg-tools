# Running the logger

The logger is one program, `dgg-logger`, whose commands share one data
directory:

| Command | Job | Runs |
|---|---|---|
| `capture` | Connects to dgg chat and writes everything to the journal | all the time |
| `index` | Reads the journal into `index.sqlite` | all the time, with `--follow` |
| `compact` | Packs each finished day into one compressed archive | every hour |
| `gaps` | Lists when no capture was connected | when you want |
| `backfill` | Fetches missed messages from public logs | after an outage |
| `sync`, `serve` | Copy the journal between hosts | when moving hosts |
| `install`, `uninstall`, `status`, `supervise` | Run, install and inspect the rest | when you want |

`dgg-supervisor` keeps capture and index running, restarts them after
failures, and runs compact; it's `dgg-logger supervise` without a window,
for the logon task and the service. In Task Manager, both show as
"dgg-tools: ..."; the Details tab's Command line column tells the commands
apart.

The commands that work on the data (`capture` to `serve`) take `--data`,
which defaults to the installed logger's.

An install lives in one folder:

```
%LOCALAPPDATA%\dgg-tools\        (C:\ProgramData\dgg-tools for a machine install)
  bin\<time>-<version>\    one build per install; older ones are pruned
  data\
    journal\
      live\                today's raw capture, one file per capture per day
      archive\             finished days, compressed
      backfill\            messages fetched from other logs, kept apart
      control\             each running capture's status
    index.sqlite           the searchable index; can be rebuilt from the journal
  logs\                    capture.log, index.log, compact.log, supervisor.log
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

### Upgrading, switching, and the old scripts

Build, then run `install` again, the same way. Nothing is missed: a capture
from the new build starts first (the "bridge", logging to `bridge.log`),
the old install hands over to it and stops, and then the new install starts
and takes over from the bridge. Old builds are pruned, keeping three.

Running `install` with the other kind (with or without `--machine`)
switches to it the same way, moving the data and logs to the new folder.
Anything involving a machine install needs an administrator prompt.

An install made by the PowerShell scripts that came before (`install.ps1`)
is taken over the same way, its tasks and old builds removed.

If anything goes wrong, `install` says what, and the capture that was
running (or the bridge) keeps running.

## Status

```powershell
target\release\dgg-logger status
target\release\dgg-logger status --watch
```

It finds the installed logger and shows:

- the running programs: memory, CPU, and which build;
- each capture's health: healthy, connecting, or STALE once its heartbeat
  stops;
- the journal: today's files and their last write, days waiting for
  compact, archives, backfill;
- the index, free disk space, and the ends of the logs.

`--watch` keeps it open, redrawing every 2 seconds (`--interval` to change)
with how fast today's journal is growing and a log filling the rest of the
window. Keys: `1`-`4` show the capture, index, compact or supervisor log,
`r` redraws, `e` opens the data folder, `q` quits.

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
service, and deletes the binaries. Only the data and logs are left, and the
folder holding them; a machine install's get ordinary permissions back,
owned by you, so you can move or delete them without elevation. A machine
install needs an administrator prompt to remove. Delete the data and logs
by hand to remove everything.
