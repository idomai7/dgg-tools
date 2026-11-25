# Running the logger

The logger is one program, `dgg-logger`, whose commands share one data
directory:

| Command | Job | Runs |
|---|---|---|
| `dgg-capture` | Connects to one dgg channel's chat, from `dgg-channels.jsonc`, and writes everything to the journal | all the time, one for each channel in `dgg-channels.jsonc` |
| `index` | Reads each dgg channel's journal into `index-dgg-<name>.sqlite`, and each Kick channel's into `index-kick-<slug>.sqlite` | all the time, with `--follow`: once 500 records are waiting (at most once a minute), or every 10 minutes |
| `compact` | Packs each finished day into one compressed archive, Kick's channels' too; with `--early`, also what stopped captures left on today | every hour; `--early` a minute after start |
| `dgg-gaps` | Lists when no capture of a dgg channel was connected | when you want |
| `dgg-backfill` | Fetches destiny.gg's missed messages from public logs | after an outage |
| `sync`, `serve` | Copy the journal between hosts | when moving hosts, or every 15 minutes from a peer |
| `kick-ids` | Records Kick channels' ids in `kick-channels.jsonc`, for `kick-capture` | when adding a channel |
| `kick-capture` | Connects to one Kick channel's chat, from `kick-channels.jsonc`, and writes everything to the journal | all the time, one for each channel in `kick-channels.jsonc` |
| `install`, `uninstall`, `status`, `config`, `supervise` | Run, install and inspect the rest | when you want |

`dgg-supervisor` keeps the captures and index running (and serve, given
its config), restarts them after failures, and runs compact (and sync,
given a peer); it's `dgg-logger supervise` without a window, for the logon
task and the service. It runs a `dgg-capture` for each channel in
`dgg-channels.jsonc` and a `kick-capture` for each in
`kick-channels.jsonc`, each on its own, so one failing leaves the others,
and follows the lists as they change (see [Channels](#channels)). In Task
Manager, both show as "dgg-tools: ..."; the Details tab's Command line
column tells the commands apart.

The commands that work on the data (`dgg-capture` to `serve`) take `--data`,
which defaults to the installed logger's.

An install lives in one folder:

```
%LOCALAPPDATA%\dgg-tools\        (C:\ProgramData\dgg-tools for a machine install)
  bin\<time>-<version>+<commit>\  one build per install; older ones are pruned
  data\
    journal\
      dgg\<name>\          each dgg channel's journal:
        live\              today's raw capture, one file per capture per day
        archive\           finished days, compressed
        backfill\          messages fetched from other logs, kept apart
        control\           each running capture's status
      kick\<slug>\         each Kick channel's journal, laid out the same
    index-dgg-<name>.sqlite  each dgg channel's searchable index; can be
                           rebuilt from the journal
    index-kick-<slug>.sqlite  each Kick channel's, likewise
  logs\                    index.log, compact.log, supervisor.log, sync.log,
                           migrate.log, a dgg-capture.<name>.log for each
                           dgg channel, and a kick-capture.<slug>.log for
                           each Kick channel
  install.env.json         the install's config, secrets included: the mesh
                           this node is in, other hosts to sync from, the
                           mesh's CA (`dgg-logger config` shows it without
                           its secrets)
  nodes                    the mesh's node list, if any
  dgg-channels.jsonc       the dgg channels to capture (destiny.gg's alone
                           at first), edited by hand
  kick-channels.jsonc      the Kick channels to capture, from `kick-ids`
```

The journal is the record. The indexes can always be deleted and rebuilt with
`dgg-logger index --rebuild`, or only dgg's with `--rebuild-dgg` and only
Kick's with `--rebuild-kick`. Logs are timestamped and rotated at 10 MB,
keeping 5.

## Channels

Each service captures the channels in its list, in the install's folder.
dgg's, `dgg-channels.jsonc`, is edited by hand: a chat speaking dgg's
protocol, its WebSocket URL, and the Origin header to send (or `null` for
none). Its name is the site's domain by convention, and names its journal,
index and log:

```jsonc
{
  "channels": [
    {
      "name": "destiny.gg",
      "url": "wss://chat.destiny.gg/ws",
      "origin": "https://www.destiny.gg",
    },
    {
      "name": "example.org",
      "url": "wss://chat.example.org/ws",
      "origin": null,
    },
  ],
}
```

Kick's, `kick-channels.jsonc`, holds each channel's slug and ids, which
`dgg-logger kick-ids <slug>` looks up and adds.

Both are read every minute: a channel added is captured within the
minute, and one removed stops, its journal kept (and still synced,
indexed and compacted). A changed URL or Origin, or changed ids,
reconnects that channel's capture alone. A list that can't be read
changes nothing; the supervisor's log and `status` say why. A channel a
peer has and this host doesn't list is synced all the same. Only
destiny.gg's chat has public logs, so `dgg-backfill` refuses the others,
and their gaps stay gaps.

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
it. Running `dgg-backfill`, `dgg-gaps` or `index` by hand against a machine
install's data needs an administrator prompt.

`--root`, `--data` and `--logs` put things elsewhere.

### Upgrading and switching

Build, then run `install` again, the same way. Nothing is missed: for each
dgg channel, a capture from the new build starts first (the "bridge", its
lines in the channel's `dgg-capture.<name>.log` marked `(bridge)`), the old
install hands over to them and stops, and then the new install starts and
takes over from the bridges. Every dgg channel's must be healthy in time,
or the install stops there. Old builds are pruned, keeping three.

Each Kick channel's capture is bridged the same way, each bridge's lines
in its channel's `kick-capture.<slug>.log`. If one isn't
healthy in time, `install` warns that that channel may miss a few
seconds, and carries on: dgg's captures come first. It waits for the new
install's capture of each channel too, warning of any not healthy in
time, and names each channel's healthy capture when it's done.

The bridges start 5 seconds apart, dgg's first, and so do the new
install's captures, so the handoffs are spread out and no chat server sees
every channel reconnect at once. `dgg-channels.jsonc` and
`kick-channels.jsonc` move with the install's folder. Either one that
isn't there is written before the bridges start: from the plain
`dgg-channels` or `kick-channels` of an install from before they were
JSONC, if there's one, else new. The plain one is renamed `.old` once the
old install has stopped.

Running `install` with the other kind (with or without `--machine`)
switches to it the same way, moving the data and logs to the new folder.
Anything involving a machine install needs an administrator prompt.

If anything goes wrong, `install` says what, and the capture that was
running (or the bridge) keeps running.

dgg's journals are by channel, destiny.gg's in `journal\dgg\destiny.gg\`
and its index in `index-dgg-destiny.gg.sqlite`. An install moves them
there from `journal\dgg\` and `index-dgg.sqlite` as its supervisor starts;
a live file still being written is moved at a later start, and
`migrate.log` says what moved and what was left. Kick's index is by
channel too, `index-kick-<slug>.sqlite`: an install deletes the old
`index-kick.sqlite` as its supervisor starts, and `index` builds each
channel's from its journal.

Builds `ed46061` to `52d088d` moved dgg's journal into `journal\dgg\` from `journal\` and
`index.sqlite`; later builds don't, so upgrading from a build older than
`ed46061` goes through one of those first.

Builds from `b9651ea` name dgg's commands by service: `dgg-capture`,
`dgg-backfill` and `dgg-gaps`, once `capture`, `backfill` and `gaps`.
`install` renames `capture.log` to `dgg-capture.log`.

A minute after it starts, the supervisor runs `compact --early`, which
also compacts days still open: the files of captures that have stopped
(the one before an upgrade, say) are archived then, not once the day ends.
A capture's files are left while it may still write them: while it holds
its lock, `control\<instance>.live-lock`, or, for builds from before the
lock, while its status is fresh. The rest of the day joins the archive
once it ends.

## Status

```powershell
target\release\dgg-logger status
target\release\dgg-logger status --watch
```

It finds the installed logger and shows:

- the running programs: memory, CPU, and which build (each
  capture with its channel);
- each capture, each dgg and Kick channel's: what it listens to (the dgg
  channel and its WebSocket URL, or the Kick channel), its health
  (healthy, connecting, or STALE once its heartbeat stops), and the frames in its files not
  yet compacted;
- each dgg channel's journal (today's files and their last write, days
  waiting for compact, archives, backfill) and index, and each Kick
  channel's journal today and index;
- free disk space, the peer it syncs from (and how the last round went),
  and the ends of the logs.

`--watch` keeps it open, redrawing every second (`--interval` to change)
under a title naming its build, with how fast each dgg channel's journal is
growing today and a page filling the rest of the window. The keys are
grouped:

- `l` every log at once, in time order, each line naming its log (the page
  it starts on); `r` the latest records of the capture at the top of the
  captures box (`r` again for the next capture, round every channel's),
  titled with what it listens to; `f` one channel's capture's log, dgg's
  or Kick's (`f` again for the next, and after the last, every capture's);
  Space redraws, `e`
  opens the data folder, `q` quits;
- each program's log: `1` the supervisor's, `2` migrate's, `3` every
  capture's, dgg's and Kick's, in time order, `4` compact's, `5` index's,
  `6` the gaps in the first dgg channel's capture as `dgg-gaps` reports
  them (looked at again every 30 seconds; it reads the index as `index`
  has brought it, without updating it; `6` again for the next channel's),
  `7` sync's, `8` serve's;
- the boxes: Tab and Shift-Tab move the focus between them, the page at
  the bottom too (its border turns cyan); Up, Down, Page Up and Page Down
  scroll the focused one when it's over-tall (its title says which lines
  it shows); `m` minimizes it to its title line, or restores it, the
  others taking the room.

The captures box lists dgg's and Kick's captures together, each after its
service, the unhealthy first (stale, then connecting), then the busiest,
by frames a minute over about the last minute.

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
logs. Both take `--channel`, destiny.gg's by default:

```powershell
target\release\dgg-logger dgg-gaps --min 60
target\release\dgg-logger dgg-backfill --source linkers --dry-run
target\release\dgg-logger dgg-backfill --source linkers
```

Short gaps are often already covered by the chat history dgg sends on every
reconnect; `dgg-gaps` says which ones were. Backfilled messages go in the
channel's `backfill\`, separate from what was captured, and never replace
a captured message. `--source rustlesearch` is millisecond-accurate but
limited to 200 requests a day. linkers.ooo allows 60 requests a minute,
unlimited with a logged-in session's `lk_session` cookie, given as
`--config-file <file>` holding `{"linkers_session": "<the cookie>"}`.

Don't start a history import (`backfill --from`) by hand yet: it's
meant to run for weeks and comes later.

## Syncing from other hosts

An install can also pull other hosts' journals every 15 minutes, Kick's
channels' included, so it has whatever they captured while this one was
off. Each other host runs
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

Then, on each node, with the mesh's read token (the same on every node).
Secrets are given as JSON, from a file or stdin, never as a flag or in
the environment:

```powershell
'{"mesh_token": "<the mesh token>"}' | target\release\dgg-logger install --name pi --nodes nodes --config-stdin
```

Each node syncs from every other node with a URL. The list is copied to
`nodes` in the install's folder, and `--name` and the token go in
`install.env.json`; to change the list, edit it and run `install --nodes`
again on each node (or edit `nodes` by hand: it's read before each round).
`install --no-mesh` leaves the mesh. `install --ca <ca.pem>` keeps the
mesh CA's certificate in the config: `https://` peers are then trusted on
its word alone.

### One-off peers

To also sync from a host outside the mesh, such as one being moved off:

```powershell
'{"peer_token": "<its token>"}' | target\release\dgg-logger install --peer http://other-host:8788 --config-stdin
```

That adds `other-host` to the config's `peers` (`--peer-name` names it
otherwise). `install --no-peer other-host` removes it, and `--no-peers` all
of them.

The config and node list are read before each round, so they can be edited
by hand too. If the config can't be read, the supervisor says why and syncs
as it did before. `dgg-logger config` shows the config with every secret as
`<secret>`, and anything wrong with it. In a machine install, only
administrators and the service can read it. The next `install` keeps it;
`uninstall` removes it.

Installs from before kept these in `mesh.env`, `peers\<name>.env`,
`peer.env` and `ca.pem`. The next `install` folds them into
`install.env.json` and removes them; until then, they're warned of, and
not read. Each round's output is in `sync.log`, each line after the peer's
name; `status` shows each peer's last round. The index picks up what
arrives.

### Running serve safely

On each host synced from, have the supervisor serve, with this node's
certificate (from `cert node`) and any tokens beyond the mesh's. Tokens,
like every secret, are given as JSON, in a file (`--config-file`) or on
stdin (`--config-stdin`), never as a flag or in the environment:

```powershell
$query = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32))
@{ serve = @{ query = $query } } | ConvertTo-Json | target\release\dgg-logger install --serve 0.0.0.0:8788 --serve-cert pi.pem --serve-key pi.key --config-stdin
```

That keeps the certificate, key and tokens in `install.env.json`; the
supervisor serves with them from its next start (`serve.log`, and `9` in
`status --watch`), its read token the mesh's unless `serve.read` is given.
`install --serve` again replaces them; `install --no-serve` stops serving.

To serve by hand instead, give `serve` its tokens the same way:

```powershell
$token = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32))
@{ tokens = @{ read = $token } } | ConvertTo-Json | Set-Content serve.json
target\release\dgg-logger serve --listen 0.0.0.0:8788 --config-file serve.json
```

Keep `serve.json` where only you can read it. `sync` takes the peer's read
token the same way, as `{"token": "..."}`.

- Without a certificate, `serve` speaks plain HTTP, so the token crosses
  the network readable. Reach it only over something private: a VPN such
  as Tailscale or WireGuard, or an SSH tunnel. Don't open the port to the
  internet.
- That token only reads, which is all sync needs. A second one,
  `tokens.control`, lets a capture on another host push its status there,
  which makes that host's captures retire. Set it only while moving
  capture to a new host (`capture --peer`, given `{"token": "<it>"}`), and
  give it only to that host. Tokens must be 32 characters or more, and no
  two may be the same.
- Sync trusts the peer for what its journal says: whatever it serves is
  added here. Sync from your own hosts only. It doesn't trust the peer
  for size, though: it refuses a manifest over 16 MB and a file over 4 GB,
  takes no more of a file than the manifest lists, checks archives
  against their SHA-256, and doesn't follow redirects.
- `serve` handles 8 requests at once and turns away the rest.
- A third token, `tokens.query`, reads only Kick's history and live chat
  (`/kick/v0/channels/...`), for kick-mod.
- Each service's routes start with its name and its API's version, and
  are by channel: `/dgg/v0/channels` lists dgg's channels, and
  `/dgg/v0/channels/<name>/manifest` and `.../files/...` are a channel's
  for sync, `/dgg/v0/channels/<name>/control/...` for its capture
  handoff, and likewise `/kick/v0/channels/...`. `v0` may change
  incompatibly from one build to the next, so hosts that sync from each
  other should run the same build, until a stable `v1`. A host without a
  service answers 404, which sync takes as nothing to pull; so does one
  on a build from before channels, so upgrade both hosts together.

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
