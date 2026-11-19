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
- **Lossless:** the raw client delivers every message from every channel it's subscribed to.
- **Writes:** only through the official public API, with a user's OAuth token. Built now, though the webhooks are deferred.

## Known
### Centrifugo (probed 2026-11-19)
Two minutes on a busy live channel, plus an offline one (`destiny`), from a scratch probe.
- **Connecting:** no login, and no `Origin` header is needed anywhere. reqwest is accepted on `web.kick.com`.
  1. `POST https://web.kick.com/api/v1/realtime/connection` with `{"client": {"id": <uuid>, "type": "web"}, "capabilities": {"accepted_providers": [{"provider": "centrifugo"}]}}`. It answers `{data: {connections: [{provider: "centrifugo", credentials: {url}}], mode: "websocket"}, message: "success"}`. The `url` was `wss://realtime.us-west-2.platform.kick.com/connection/websocket`.
  2. `POST https://web.kick.com/api/v1/realtime/auth/connection` with `{"client_id": <the same uuid>}`. It answers `{data: {token}}`, an HS256 JWT for `sub: guest:<uuid>` that lasts 45 minutes.
  3. Open the socket and send `{"id": 1, "connect": {"token": <token>, "name": "js"}}`. The reply was `{client, version: "6.9.7 PRO", expires: true, ttl: 2699, ping: 25, pong: true}`.
- **Server:** Centrifugo PRO, the commercial build. It speaks the open-source JSON protocol.
- **Keepalive:** the server sends `{}` every 25 s, and the client answers each with `{}`.
- **Token refresh:** `expires: true` with a `ttl`, so the token has to be refreshed before it runs out.
- **Channels:** subscribing to `chatrooms.<chatroom id>.v2` answers `{"subscribe": {}}`.
  - `chatrooms.<id>`, `chatroom_<id>`, `channel.<channel id>` and `channel_<channel id>` were accepted too, but carried nothing in 2 minutes. Whether they're real channels or the server accepts any name is unknown.
  - Reportedly at most 128 subscriptions per connection (error 106 beyond that).
- **No history or recovery:** the subscribe reply has no `recoverable`, `offset` or `epoch`, and `history` answers `108 not available`. Whatever arrives while disconnected is lost.
- **Frames:** one push per frame: `{"push": {"channel", "pub": {"data": {"event", "data": "<JSON as a string>"}, "tags": {"event"}}}}`.
- **Events seen:**
  - `App\Events\ChatMessageEvent` (1577):
    - `id` (uuid), `chatroom_id`, `content`, `type` (`message` or `reply`), `created_at`, `metadata.message_ref`.
    - `sender`: `id`, `username`, `slug`, and `identity` (`color`, `badges` with `type` such as `moderator`, `subscriber`, `verified` or `founder`, plus `badges_v2`).
    - `created_at` is whole seconds only. `message_ref` looks like a millisecond timestamp, perhaps set by the sender's client.
  - `App\Events\UserBannedEvent` (15): `id`, `user` (`id`, `username`, `slug`), `banned_by`, `permanent`, `duration`, `expires_at`.
    - A timeout is `permanent: false` with a duration, apparently in minutes.
    - All 15 were 1-minute timeouts by `banned_by.id` 0, named as the broadcaster: probably a bot or automatic action.
  - `App\Events\MessageDeletedEvent` (6): `id`, `message.id`, and for Kick's AI moderation `aiModerated: true` and `violatedRules` (e.g. `hate`).
    - Nothing says who deleted the message.
- **Chatroom id:** different from the channel id and the user id (for `destiny`: chatroom 1764849, channel 1772249, user 1823122).
  - It's on the unofficial `GET https://kick.com/api/v2/channels/<slug>` (`chatroom.id`).
  - Cloudflare refuses reqwest there with `{"error": "Request blocked by security policy."}`, but lets curl through, so it's probably blocking on the TLS fingerprint.

### Official API
- App token via `POST https://id.kick.com/oauth/token` (`client_credentials`).
- Channel lookup via `GET /public/v1/channels?slug=` (gives `broadcaster_user_id`).
- Webhooks: subscribe with `POST /public/v1/events/subscriptions` (e.g. `chat.message.sent`). The webhook URL is set in the app settings.
- Webhook verification: RSA PKCS#1 v1.5 SHA-256 over `messageId.timestamp.body`, using the `Kick-Event-*` headers. The public key comes from `/public/v1/public-key`.
- Webhook payload: `content`, with emotes inline as `[emote:ID:NAME]`, plus `sender.identity` and `created_at`.
- Cloudflare has reportedly refused some clients, with 403s on `POST /public/v1/chat` and on channel lookups (KickDevDocs issues #281, #434).

## Open questions
- The other events a chatroom carries: pins, unbans, subscriptions and gifts, chatroom settings (slow mode and the like), going live and offline. And whether the `channel.<id>` channels carry any of them.
- Whether a delete or a ban made by a moderator says who made it.
- Where to get a chatroom id from a slug without the unofficial lookup. Perhaps the official channel lookup has it; otherwise the user enters it once and the tool keeps it.
- Whether the official API refuses reqwest/rustls clients.
- For the webhooks: whether an app token may subscribe to any channel's webhooks.

## The webhook probe
It lives on the `kick` branch (`viewer/examples/kick.rs`, with `.env.example`). It compiles but hasn't been run.
- **Running it needs:**
  - a Kick app with webhooks turned on;
  - a tunnel such as `cloudflared tunnel --url http://localhost:8787`;
  - `.env` filled in from `.env.example`.

  Then, on the `kick` branch, run `cargo run -p viewer --example kick -- <slug>`.
