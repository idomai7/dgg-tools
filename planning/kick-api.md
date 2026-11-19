# kick-api

Kick's APIs, kept lightweight for any tool to depend on: [kick-mod](kick-mod.md) now, the omnichat later. Kick only, and similar in scope to `dgg-api`. See the [overall rules](README.md).

## Scope
- Reading chat over Kick's realtime socket.
- Kick's official public API (`https://api.kick.com/public/v1`): OAuth tokens, channel lookup, sending chat, deleting messages, and banning, timing out and unbanning.
- Each part behind its own feature, as in `dgg-api`.

## Decisions
- **Read path:** Centrifugo, which kick.com's own web client has used since 2026-10.
  - The client follows Centrifugo's open-source client protocol (github.com/centrifugal/centrifugo, github.com/centrifugal/protocol). Only Kick's configuration of it is probed.
  - Not Pusher: it's the legacy path that Kick's site no longer uses, and its key changes without notice (last on 2026-10-17, to `34bf7a0ff419a2a775b9` on cluster `us2`).
- **Second read backend:** the official webhooks, in case the socket closes off. Deferred.
  - They need a public HTTPS receiver with a publicly trusted certificate, which the mesh's private CA can't provide.
- **Writes:** only through the official public API, with a user's OAuth token. Built now, though the webhooks are deferred.

## Known
- **Realtime connection details:** `POST https://web.kick.com/api/v1/realtime/connection`, with no login, answers `{data: {connections: [{provider, credentials}], mode}}`. Offered both providers, it picks `centrifugo`. Reported by other projects in 2026-10; untried here.
- **Channels:** `chatrooms.{chatroom_id}.v2`, public. Reportedly at most 128 subscriptions per connection (error 106 beyond that).
- **Official API:**
  - App token via `POST https://id.kick.com/oauth/token` (`client_credentials`).
  - Channel lookup via `GET /public/v1/channels?slug=` (gives `broadcaster_user_id`).
  - Webhooks: subscribe with `POST /public/v1/events/subscriptions` (e.g. `chat.message.sent`). The webhook URL is set in the app settings.
  - Webhook verification: RSA PKCS#1 v1.5 SHA-256 over `messageId.timestamp.body`, using the `Kick-Event-*` headers. The public key comes from `/public/v1/public-key`.
  - Webhook payload: `content`, with emotes inline as `[emote:ID:NAME]`, plus `sender.identity` and `created_at`.
  - Cloudflare has reportedly refused some clients, with 403s on `POST /public/v1/chat` and on channel lookups (KickDevDocs issues #281, #434).

## Open questions
- What the connection endpoint returns for Centrifugo: the socket URL, and whether there's a token and how long it lives.
- Whether anonymous connections are allowed, and whether `Origin` is checked.
- Whether history and recovery are on, so that a reconnect can fill what it missed.
- Which events a chatroom channel carries besides messages: deletes, bans, timeouts, pins, mod actions.
- Whether the official API refuses reqwest/rustls clients.
- For the webhooks: whether an app token may subscribe to any channel's webhooks.

## The webhook probe
It lives on the `kick` branch (`viewer/examples/kick.rs`, with `.env.example`). It compiles but hasn't been run.
- **Running it needs:**
  - a Kick app with webhooks turned on;
  - a tunnel such as `cloudflared tunnel --url http://localhost:8787`;
  - `.env` filled in from `.env.example`.

  Then, on the `kick` branch, run `cargo run -p viewer --example kick -- <slug>`.
