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

## Install config
Everything an install is configured with, secrets included, in one file,
`install.env.json` in its root, replacing `mesh.env`, `peers\<name>.env`
and `ca.pem`. Planned 2026-11-25.
- **What's in it:** every field optional; certificates are PEM, as text.

  ```json
  {
    "ca": "-----BEGIN CERTIFICATE-----...",
    "mesh": { "node": "pi", "token": "<secret>" },
    "peers": { "old": { "url": "https://...", "token": "<secret>" } },
    "serve": {
      "listen": "0.0.0.0:8788",
      "cert": "-----BEGIN CERTIFICATE-----...",
      "key": "<secret>",
      "tokens": { "read": null, "control": null, "query": "<secret>" }
    }
  }
  ```

  - `ca`: the mesh CA's certificate: `https://` peers are trusted on its
    word alone.
  - `mesh`: this node's name in `nodes` and the mesh's read token. The node
    list stays a file of its own, `nodes`, the same on every node and not
    secret.
  - `peers`: one-off peers, by name.
  - `serve`: with it, the supervisor runs `serve` (see the steps). `read`
    is the mesh's token if left out; `control` is for moving capture
    between hosts only.
- **Secrets:** each token and the key is a `Secret` (see the
  [rules](README.md)). The file can be read only by administrators and the
  service in a machine install, as `mesh.env` is now.
- **Seeing it:** `dgg-logger config` prints the config with every secret
  as `<secret>`, so it can be checked, by a person or an agent, without
  the secrets being read. `status` shows the same.
- **Writing it:** `install` writes it whole, to a new file renamed over the
  old, from its flags and from `--config-file` or `--config-stdin` for the
  secrets; `--no-mesh`, `--no-peer` and `--no-serve` take parts out.
- **Reading it:** the supervisor reads it before each round, as it does
  the peers now. One that doesn't parse is logged, and the last that did
  is kept.
- **Passing it on:** each command the supervisor runs gets only its part,
  as JSON on its stdin (`--config-stdin`): `sync` a peer's URL and token;
  `serve` its section.
- **Migration:** `install` folds `mesh.env`, `peers\*.env`, `peer.env` and
  `ca.pem` into the file, then removes them; if the file has a part
  already, it's kept and the old file's is warned of. The supervisor warns
  of any old files left, by name, without reading them.
- **Steps,** each a pause for review:
  1. Config JSON in the logger: `Secret` and the loader in `common`, and
     `sync`, `serve`, `capture` and `backfill` reading their tokens with
     `--config-file` or `--config-stdin`, their token flags and
     environment variables gone. The supervisor passes `sync` its token on
     stdin, still from the old files. `deploy`'s own `--mesh-token` and
     `--peer-token` move with step 2; `cert`'s `DGG_CA_KEY` and
     `DGG_CA_CERT` go, its flags' defaults staying. Done 2026-11-25.
  2. `install.env.json`, `dgg-logger config`, and the migration.
  3. `serve` under the supervisor, from the file's `serve` section, its
     certificate and key passed on stdin, restarted like capture, and
     shown by `status`.
  4. kick-mod switches to the logger (see [kick-mod](kick-mod.md)), its
     query token in its own config JSON.

## Status
- The supervisor syncs from every peer in turn: the mesh's node list (`nodes` +
  `mesh.env`) and one-off `peers\<name>.env` files. A mesh of three settles in
  three rounds (tested).
