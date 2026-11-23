# deploy

The mesh of nodes running the logger. See the [overall rules](README.md) for the
deployment principles, and `deploy/README.md` for operating it.

## Decisions
- Nodes: a DigitalOcean droplet, an Oracle Always Free VM and a Raspberry Pi at
  home, each capturing, plus any others added later.
- Every node syncs from every reachable node, as listed in a node list kept in
  the repo. Nodes behind NAT push their own captures.
- `serve` gets TLS from a private CA kept on the PC.
- The plan, in steps: multiple peers, TLS, push, Linux builds, a systemd
  install, OpenTofu with a Rust `dgg-deploy`, `status --mesh`, then rollout.
- The logger's Kick capture (see [logger](logger.md#kick)) runs on the same
  nodes, under the same supervisor, from a `kick-channels` list in the
  install. `serve` also answers kick-mod, with a query token of its own.

## Status
- The supervisor syncs from every peer in turn: the mesh's node list (`nodes` +
  `mesh.env`) and one-off `peers\<name>.env` files. A mesh of three settles in
  three rounds (tested).
