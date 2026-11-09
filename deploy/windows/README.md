# Running the logger on Windows

The logger is a few small programs that share one data directory:

| Program | Job | Runs |
|---|---|---|
| `capture` | Connects to dgg chat and writes everything to the journal | all the time |
| `index` | Reads the journal into `index.sqlite` | all the time, with `--follow` |
| `compact` | Packs each finished day into one compressed archive | once a day |
| `gaps` | Lists when no capture was connected | when you want |
| `backfill` | Fetches missed messages from public logs | after an outage |

Everything lives under one folder. These docs use
`%LOCALAPPDATA%\dgg-tools`, which is also where `install.ps1` puts things,
so you can start in a shell and switch to scheduled tasks later without
moving any data.

```
%LOCALAPPDATA%\dgg-tools\
  bin\<build>\     a copy of the programs (see "Why copy the programs")
  data\
    journal\
      live\        today's raw capture, one file per capture per day
      archive\     finished days, compressed
      backfill\    messages fetched from other logs, kept apart
      control\     each running capture's status
    index.sqlite   the searchable index; can be rebuilt from the journal
  logs\            only used by the scheduled tasks
```

The journal is the record. The index can always be deleted and rebuilt
with `index --rebuild`.

## Running it in a shell

You need Rust (`cargo`) and two PowerShell windows. Nothing needs
administrator rights.

### 1. Build and copy

From the repository:

```powershell
cargo build --release -p logger
$root = "$env:LOCALAPPDATA\dgg-tools"
$bin = "$root\bin\shell-$(Get-Date -Format yyyyMMdd-HHmmss)"
New-Item -ItemType Directory -Force $bin | Out-Null
Copy-Item target\release\capture.exe, target\release\index.exe,
  target\release\compact.exe, target\release\gaps.exe,
  target\release\backfill.exe $bin
```

### 2. Start capture (window 1)

```powershell
$root = "$env:LOCALAPPDATA\dgg-tools"
$bin = (Get-ChildItem "$root\bin" -Directory | Sort-Object Name | Select-Object -Last 1).FullName
& "$bin\capture.exe" --data "$root\data"
```

It prints `capturing as <host>-<number> into ...` and then stays quiet. It
reconnects by itself if the connection drops.

### 3. Start the index (window 2)

```powershell
$root = "$env:LOCALAPPDATA\dgg-tools"
$bin = (Get-ChildItem "$root\bin" -Directory | Sort-Object Name | Select-Object -Last 1).FullName
& "$bin\index.exe" --data "$root\data" --follow
```

It prints a summary every minute while messages arrive. Run only one index
at a time.

### 4. Compact once a day

Days are in UTC, and a day can be compacted an hour after it ends. Any time
the next day, in a third window or either of the others after stopping it:

```powershell
& "$bin\compact.exe" --data "$root\data"
```

It's safe to run while capture and index are running, and safe to run when
there's nothing to do. Use `--dry-run` to see which days it would pack.

### Stopping

Press **Ctrl-C** in each window. Capture saves and closes its file cleanly.
Closing the window or a crash also loses nothing already written: at most
the last second, and the journal reader skips a half-written last line.

### Checking on it

```powershell
powershell -ExecutionPolicy Bypass -File deploy\windows\status.ps1
```

It shows the scheduled tasks, the running programs, each capture's health
(healthy, connecting, or stale when its heartbeat stopped), the journal
(today's files and when they were last written, days waiting for compact,
archives, backfill), the index, free disk space and the end of the logs.
The log section stays empty for shell runs; their output is in the
windows.

Add `-Watch` to keep it open: it redraws every 2 seconds (`-Interval` to
change), shows how fast today's journal is growing, and fills the rest of
the window with a log. Keys: `1` `2` `3` show the capture, index or compact
log, `r` redraws, `e` opens the data folder, `n` opens the log in Notepad,
`q` quits.

### Upgrading without missing messages

1. Build and copy again (step 1). Each build goes into its own folder.
2. Start the new capture in a new window (step 2). Leave the old one
   running.
3. About 30 seconds after the new one is connected, the old one prints
   `handed off to <new instance>` and exits by itself.
4. Stop the old index with Ctrl-C and start the new one (step 3).

Two captures can run side by side for as long as you like; the index merges
them without duplicates. The one started last wins the handoff.

## After an outage

If the PC was off or asleep, see what was missed and fill it from public
logs:

```powershell
& "$bin\gaps.exe" --data "$root\data" --min 60
& "$bin\backfill.exe" --data "$root\data" --source linkers --dry-run
& "$bin\backfill.exe" --data "$root\data" --source linkers
```

Short gaps are often already covered by the chat history dgg sends on every
reconnect; `gaps` says which ones were. Backfilled messages go in
`journal\backfill\`, separate from what was captured, and never replace a
captured message. `--source rustlesearch` is millisecond-accurate but limited
to 200 requests a day.

Don't start a history import (`backfill --from`) by hand yet: it's meant to
run for weeks and comes later.

## Keeping the PC awake

Capture stops while the PC sleeps. While plugged in:

```powershell
powercfg /change standby-timeout-ac 0
```

## Why copy the programs

Windows locks a running `.exe`. If capture ran straight from
`target\release`, the next `cargo build` would fail until you stopped it,
which would leave a hole in the capture. Running a copy keeps the build
folder free. Old copies under `bin\` can be deleted once nothing runs from
them.

## Later: scheduled tasks

`install.ps1` does all of the above as scheduled tasks for your account,
starting at logon, with output in `logs\`:

```powershell
powershell -ExecutionPolicy Bypass -File deploy\windows\install.ps1
```

If a shell capture is running when you install, the task's capture takes
over from it and the shell one exits on its own. Stop the shell index
yourself.

Run the same command again to upgrade: the new capture starts beside the
old one, which hands off and exits, so nothing is missed.

### A machine install

`-Scope Machine` (from an administrator PowerShell) runs the tasks as
SYSTEM from startup instead, whether or not anyone is logged on, in
`C:\ProgramData\dgg-tools`. That folder is then writable only by
Administrators and SYSTEM, because anyone who could change it could run code
as SYSTEM. Your account can still read the data and logs, but running
`backfill`, `gaps` or `index` by hand against it needs an administrator
shell.

### Switching between the two

Run `install.ps1` with the scope you want, from an administrator
PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -File deploy\windows\install.ps1 -Scope Machine
powershell -ExecutionPolicy Bypass -File deploy\windows\install.ps1 -Scope User
```

It stops the old install, moves its data and logs to the new one's folder,
deletes the old binaries and starts the new tasks. Capture is stopped for
the few seconds that takes, and the history dgg sends on reconnect covers
that. Data from shell runs in `%LOCALAPPDATA%\dgg-tools\data` moves the
same way; stop any shell runs that weren't started from `bin\` first.

If a move fails partway, the old tasks are already gone: fix what the error
says and run it again, or run it with the old scope to go back.

### Uninstalling

```powershell
powershell -ExecutionPolicy Bypass -File deploy\windows\uninstall.ps1
```

It stops everything running from the install's `bin\` (shell runs from
there too), removes the scheduled tasks and their `\dgg-tools\` folder in
Task Scheduler, and deletes the binaries. Only the data, `logs\` and the
folder holding them are left. For a machine install (which needs an
administrator PowerShell) they get ordinary permissions back, owned by you,
so you can move or delete them without elevation. Delete them by hand to
remove everything.
