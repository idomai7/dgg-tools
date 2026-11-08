# dgg-tools planning

A Cargo workspace of tools built around destiny.gg chat and its endpoints. Every tool that talks to dgg chat goes through the shared `dgg-chat` crate.

## Layout
The layout is flat: each crate is a direct child of the root.
- `dgg-chat/`: the shared socket client and event types.
- `dgg-journal/`: the journal file format, with a writer and a reader.
- `logger/`: binaries `capture`, `index`, `serve`, `sync`, `backfill` and `compact`.
- `viewer/`: the read-only chat viewer, formerly dggkick.

## Decisions
- **Stack:** Rust. Node isn't installed on this machine.
  - axum, tokio-tungstenite, reqwest, rustls (ring).
  - Pages are plain JS with no build step.
- **Style:** rustfmt with `max_width = 80` and `tab_spaces = 2` (`rustfmt.toml`).
- **Deployment:** must stay portable across Windows and Linux.
- **Logger storage:** an append-only raw journal is the source of truth. SQLite (rusqlite, `bundled`) is an index that can be rebuilt from it.
- **Kick:** deferred (see below).

## Roadmap
1. **Logger** (current focus): see the next section.
2. **Read-only viewer:** in the style of omnichat, backfilling from the journal.
3. **Others, not yet ordered:**
   - read-write chat client;
   - embed logger;
   - embed transcriptions;
   - archival;
   - clipping tools;
   - links/attachments viewer (like linkers.ooo).

## Logger design
- **Journal layout:**
  - `journal/live/YYYY-MM-DD/<instance>.jsonl`: one file per capture instance per day. These are the hot files.
  - `journal/archive/YYYY/MM/YYYY-MM-DD.jsonl.zst` plus a `manifest.json` per month. These are cold.
  - `journal/control/<instance>.json`: heartbeats.
  - Hot paths only ever list `live/`.
- **Days:** UTC.
- **Line format:** shared `recv` (ms), `inst` and `conn`, plus exactly one body key:
  - `"frame":"<exact text frame>"`;
  - `"binary":"<base64>"`;
  - `"open":"<url>"`;
  - `"close":"<why>"`.

  Event names are never interpreted at this layer, so dgg adding or removing events can't break the journal.
- **capture:** dgg-chat into the journal, and nothing else, so it stays tiny and rarely changes.
  - **In-place upgrade:** start the new binary. Once it's healthy, the old instance writes for a 30 s overlap and then retires itself.
  - **Across hosts:** the same rule works with `--peer <url>`.
- **index:** journal into SQLite.
  - The dedupe key is `(ts, nick, data)`. It was verified with `dgg-chat/examples/history_check.rs`: `HISTORY` is a JSON array of the last 150 `"MSG {...}"` frames, byte-identical to the live frames.
  - It also records `src` (where each row came from), gaps, and per-file checkpoints, and has an FTS5 table for search.
  - `--rebuild` replays the whole journal.
- **serve / sync:** read-only HTTP access to the journal, and pulling another host's journal. Together they handle migrating to a new host and running a warm standby.
- **backfill:** fills gaps from a peer's journal, from `HISTORY`, or from public archives (rustlogs etc.).
  - Rows from public archives match on the same nick and data within ±2 s, and native rows win.
  - The sources still need probing.
- **compact:** merges a closed day into one archive file, dedupes and zstd-compresses it, and verifies the result before deleting the originals.

## destiny.gg chat (from chat-gui: https://github.com/destinygg/chat-gui)
- **Socket:** `wss://chat.destiny.gg/ws`. Each frame is `EVENT {json}`, split on the first space (`assets/chat/js/source.js`).
  - Answer `PING` with `PONG <data>`.
  - The handlers live in `assets/chat/js/chat.js`: MSG, NAMES, HISTORY, JOIN/QUIT, MUTE/BAN, BROADCAST, SUBSCRIPTION/GIFTSUB/MASSGIFT/DONATION, and others.
- **MSG fields:** `nick`, `data`, `timestamp`, `features` (flair names), `roles`, `subscription`, `watching`.
- **Origin:** chat-gui's dev proxy forces `Origin: https://www.destiny.gg`. Anonymous connections work with or without it (tested).
- **Emotes and flairs:**
  - Emotes: https://cdn.destiny.gg/emotes/emotes.json (`prefix`, `image[0].url`) plus `emotes.css`.
  - Flairs: https://cdn.destiny.gg/flairs/flairs.json (`name`, `label`, `priority`, `color`, `hidden`, `image[0].url`) plus `flairs.css`.

## Status
- **Workspace restructure:** done.
  - The dgg probe is at `dgg-chat/examples/probe.rs`.
  - `viewer` is a placeholder stub.
- **`dgg-chat`:** a minimal client (reconnect with backoff, PING/PONG, idle timeout). The typed events and assets are still to do.
- **`dgg-journal`:** live files, plus a writer and a reader. The `.zst` archives and manifests come with `compact`.
- **Next:** `logger capture`, then `index`.

## Deferred: Kick
The Kick probe lives on the `kick` branch (`viewer/examples/kick.rs`, with `.env.example`). It compiles but hasn't been run.
- **Approach:** official webhooks.
  - App token via `POST https://id.kick.com/oauth/token` (`client_credentials`).
  - Channel lookup via `GET /public/v1/channels?slug=` (gives `broadcaster_user_id`).
  - Subscribe with `POST /public/v1/events/subscriptions` (`chat.message.sent`). The webhook URL is set in the app settings.
- **Verification:** RSA PKCS#1 v1.5 SHA-256 over `messageId.timestamp.body`, using the `Kick-Event-*` headers. The public key comes from `/public/v1/public-key`.
- **Payload:** `content`, with emotes inline as `[emote:ID:NAME]`, plus `sender.identity` and `created_at`.
- **Open question:** whether an app token may subscribe to any channel.
- **Fallback:** the unofficial Pusher socket on `chatrooms.{id}.v2`, with key `34bf7a0ff419a2a775b9` on cluster `us2` (changed 2026-10-17).
- **Running the probe needs:**
  - a Kick app with webhooks turned on;
  - a tunnel such as `cloudflared tunnel --url http://localhost:8787`;
  - `.env` filled in from `.env.example`.

  Then, on the `kick` branch, run `cargo run -p viewer --example kick -- <slug>`.
