# deploy

The mesh of nodes running the logger. See the [overall rules](README.md) for
the deployment principles, and `deploy/README.md` for operating it.

## Decisions
- **Nodes:** a DigitalOcean droplet, an Oracle Always Free VM and a
  Raspberry Pi at home, each capturing, plus any added later. Two Windows
  hosts run it today.
- **Sync:** every node syncs from every reachable node, from a node list
  (`nodes`) kept in the repo and the same on every node. Nodes behind NAT
  push their own captures.
- **TLS:** `serve` gets its certificate from a private CA kept on the PC.
- **One supervisor** runs everything on a node, dgg's and Kick's captures
  alike (see [logger](logger.md#capture)), and `serve`, which also answers
  kick-mod with a query token of its own.

## Install config
Everything an install is configured with, secrets included, is one file,
`install.env.json`, in its root:

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

- Every field is optional; certificates are PEM text.
  - `ca`: the mesh CA's certificate; `https://` peers are trusted on its
    word alone.
  - `mesh`: this node's name in `nodes`, and the mesh's read token.
  - `peers`: one-off peers, by name.
  - `serve`: with it, the supervisor runs `serve`. `read` defaults to the
    mesh's token; `control` is only for moving capture between hosts.
- Each token and the key is a `Secret` (see the [rules](README.md)). In a
  machine install, only administrators and the service can read the file.
- `dgg-logger config` prints it with every secret as `<secret>`, so a
  person or an agent can check it without reading the secrets.
- `install` writes it whole (a new file renamed over the old) from its
  flags, and from `--config-file` or `--config-stdin` for the secrets;
  `--no-mesh`, `--no-peer` and `--no-serve` take parts out.
- The supervisor reads it before each sync round; one that can't be read
  is logged, and the last good one kept. `serve`'s section is read when the
  supervisor starts. Each command gets only its part, as JSON on its stdin,
  so no secret is in an environment or on a command line.
- Only one supervisor serves at a time (`serve.lock`): during a handoff,
  the new one waits for the old one's port.

## Open
In order, from the original plan:
- Linux builds and a systemd install.
- Provisioning with OpenTofu and a Rust `dgg-deploy`.
- `status --mesh`.
- Rolling out to the other nodes, and turning sync on between the hosts
  (both need a build from the 2026-11-26 audit or later: sync checks the
  peer's `Content-Range`).
