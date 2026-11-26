# Deployments

Every rollout of the logger to the hosts, newest first: when, the build
(the commit it was built from, as `status` shows it after the version),
which hosts, and anything that went wrong. Each rollout is also an
annotated tag, `deployed/<date>-<nn>` (local dates, AEDT; `nn` from
`01`), whose message names the hosts and the note, so `git tag -n -l
'deployed/*'` lists them.
Tags are made locally; pushing them (`git push --tags`) is the user's.

**Recording one:** when the user says a rollout is done, add its entry
and tag it, in one commit. Ask which build each host runs if it isn't
clear; `dgg-logger status` names each program's build.

**Backfilled** on 2026-11-26 from the sessions' transcripts: *exact*
where the user named the build, otherwise *inferred*: the commit that was
`HEAD` when the user said they were upgrading, which is what they built
from as a rule. Rollouts before 2026-11-24 are sparsely recorded; those
between 2026-11-09 and 2026-11-23 aren't in any surviving transcript.

## Rollouts
- **2026-11-26-02,** the second host: build unknown, between `c3ca0cc`
  and `d637e59` (both hosts were upgraded by 14:38, before step 8 of dgg
  channels was built). Not tagged.
- **2026-11-26-01** `c3ca0cc` (inferred), one host: a dgg capture healthy
  on `HISTORY` alone, and an install carrying on past a dgg bridge that
  isn't healthy. The fix for the failure below.
- **2026-11-25-11** `a4c9cc8` (inferred): Kick's index per channel.
  **Failed:** omniliberal.dev, a quiet chat, sent no `MSG`, so its bridge
  never got healthy; the install then stopped its bridges, though the
  old install had already stopped, leaving nothing capturing. Worked
  around by taking the channel out of the list for the install.
- **2026-11-25-10** `1807bad` (inferred), both: status per dgg channel, and
  the channel lists as JSONC (the install converts the plain ones).
  omniliberal.dev was added to the list after it.
- **2026-11-25-09** `2fd4d7f` (inferred), both: a `dgg-capture` per
  channel, starts staggered. Some captures were left showing `STALE`
  afterwards: statuses of killed instances (swept away since `b737929`).
- **2026-11-25-08** `7dc5320` (inferred), both: dgg's journals and index
  by channel, and the move into `destiny.gg`'s. No rollback after the
  move.
- **2026-11-25-07** `d7034ed` (inferred), both: routes and sync by
  channel, rolled out alone, as builds either side of it don't sync.
- **2026-11-25-06** `17c6019` (inferred: the user said `HEAD~1` of
  `56ab980`), both: the status screen's steps 1 to 4, and each Kick
  channel's capture logging to its own file.
- **2026-11-25-05** `fbcd34b` (inferred), both: captures say what they
  listen to; the `live_files` order fixed.
- **2026-11-25-04** `65d1456` (inferred), both: one captures box in
  `status`, a capture per Kick channel.
- **2026-11-25-03** `6e2739e` (inferred), both: kick-mod's switch to the
  logger, kick-ids with reqwest. One host's migrate warned of a control
  file left from the old layout.
- **2026-11-25-02** `a3c579a` (exact), both: the old layout's code
  removed, by service done. The code was pushed after it.
- **2026-11-25-01** `52d088d` (inferred), one host then the other:
  `compact --early`.
- **2026-11-24-04** `819bf55` (inferred), both: dgg and Kick as services,
  API v0, the move into `journal/dgg/`.
- **2026-11-24-03** `c17d246` (exact), one host then the other: older
  format versions no longer read, migrate a skeleton. Checked first that
  every live file began `journal dgg.0.3`.
- **2026-11-24-02** `3780edb` (inferred), both: after the RS LF
  migration's steps 2 and 3.
- **2026-11-24-01** `72a5fbc`, then `5818cdd` (inferred): the move to RS
  LF framing. The first host migrated to records ending at RS LF, then
  the plan changed to end headers at RS LF too, so it took a smaller
  second migration while the other host did the whole one at once. Tagged
  at `5818cdd`.
- **2026-11-09-02** `8953139` (inferred): the single-binary build, over
  the first Rust install; the PowerShell and multi-binary install code
  was removed after. The bridge's log didn't say who handed off.
- **2026-11-09-01** about `8bb90d1` (inferred, roughly): the first Rust
  install, over the PowerShell one. One of two new captures went stale
  after 30 s; only the second stayed healthy.
