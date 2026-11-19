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
Two minutes on a busy live channel, plus an offline one (`destiny`), from a scratch probe. Then a scripted run on a quiet channel of our own, where its broadcaster sent messages, deleted, pinned, timed out, banned, unbanned, toggled each chat setting and cleared chat.
- **Connecting:** no login, and no `Origin` header is needed anywhere. reqwest is accepted on `web.kick.com`.
  1. `POST https://web.kick.com/api/v1/realtime/connection` with `{"client": {"id": <uuid>, "type": "web"}, "capabilities": {"accepted_providers": [{"provider": "centrifugo"}]}}`. It answers `{data: {connections: [{provider: "centrifugo", credentials: {url}}], mode: "websocket"}, message: "success"}`. The `url` was `wss://realtime.us-west-2.platform.kick.com/connection/websocket` on the first run and `us-east-1` on the second, so it changes between connections and has to be asked for each time.
  2. `POST https://web.kick.com/api/v1/realtime/auth/connection` with `{"client_id": <the same uuid>}`. It answers `{data: {token}}`, an HS256 JWT for `sub: guest:<uuid>` that lasts 45 minutes.
  3. Open the socket and send `{"id": 1, "connect": {"token": <token>, "name": "js"}}`. The reply was `{client, version: "6.9.7 PRO", expires: true, ttl: 2699, ping: 25, pong: true}`.
- **Server:** Centrifugo PRO, the commercial build. It speaks the open-source JSON protocol.
- **Keepalive:** the server sends `{}` every 25 s, and the client answers each with `{}`.
- **Token refresh:** `expires: true` with a `ttl`, so the token has to be refreshed before it runs out.
- **Channels:** subscribing to `chatrooms.<chatroom id>.v2` answers `{"subscribe": {}}`. It carries everything below except `ChatSettingsChanged`.
  - `channel_<channel id>` carries `ChatSettingsChanged`.
  - `chatrooms.<id>`, `chatroom_<id>` and `channel.<channel id>` were accepted too, but carried nothing in either run. Whether they're real channels or the server accepts any name is unknown.
  - Reportedly at most 128 subscriptions per connection (error 106 beyond that).
- **No history or recovery:** the subscribe reply has no `recoverable`, `offset` or `epoch`, and `history` answers `108 not available`. Whatever arrives while disconnected is lost.
- **Frames:** one push per frame: `{"push": {"channel", "pub": {"data": {"event", "data": "<JSON as a string>"}, "tags": {"event"}}}}`.
- **Events seen:**
  - `App\Events\ChatMessageEvent`:
    - `id` (uuid), `chatroom_id`, `content`, `type` (`message` or `reply`), `created_at`, `metadata.message_ref`.
    - `sender`: `id`, `username`, `slug`, and `identity` (`color`, `badges` with `type` such as `broadcaster`, `moderator`, `subscriber`, `verified` or `founder`, plus `badges_v2`).
    - `created_at` is whole seconds only. `message_ref` is a millisecond timestamp about 0.5–1.2 s before the push arrives, so it's probably set by the sender's client.
    - Emotes are inline in `content` as `[emote:<id>:<name>]`. Unicode and emoji arrive as-is, and a long message (about 450 characters) arrives whole.
    - The broadcaster's link went through with `allow_links` off.
  - `App\Events\MessageDeletedEvent`: `id`, `message.id`, and for Kick's AI moderation `aiModerated: true` and `violatedRules` (e.g. `hate`). Nothing says who deleted the message, even when the broadcaster did.
  - `App\Events\UserBannedEvent`: `id`, `user` (`id`, `username`, `slug`), `banned_by`, `permanent`.
    - A timeout is `permanent: false` with `duration` in minutes and `expires_at`. A ban is `permanent: true` with neither.
    - `banned_by` names the moderator by `username` and `slug`, but its `id` is always 0, even for the broadcaster's own bans.
  - `App\Events\UserUnbannedEvent`: `id`, `user`, `unbanned_by` (with its real `id`), and `permanent`, which says whether it lifted a ban or a timeout. Removing a timeout is an unban.
  - `App\Events\PinnedMessageCreatedEvent`: the whole `message` (as in `ChatMessageEvent`, plus `thread_parent_id`), `duration` in seconds as a string (`"1200"`), and `pinnedBy` with full identity.
  - `App\Events\PinnedMessageDeletedEvent`: data `[]`. Nothing says which message or who unpinned it.
  - `App\Events\ChatroomClearEvent`: only an `id`. Nothing says who cleared it.
  - `App\Events\ChatroomUpdatedEvent` and `ChatSettingsChanged` (on `channel_<id>`, without the `App\Events\` prefix): every settings change sends both, within 0.1 s, in either order. Each carries all the settings, not just the one that changed, and neither says who changed them.
    - `ChatroomUpdatedEvent`: `id` (the chatroom), `slow_mode` (`enabled`, `message_interval` in seconds), `followers_mode` and `account_age` (`enabled`, `min_duration` in minutes), `subscribers_mode`, `emotes_mode`.
    - `ChatSettingsChanged`: `id` (uuid), `channel` (`id`, `slug`), `created_at`, and `settings` with every duration in seconds, plus `rules` and `allow_links`, which `ChatroomUpdatedEvent` lacks.
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
- The other events: subscriptions and gifts, going live and offline, title and category changes, follows, replies (seen as `type: reply` but not looked at), polls, and moderators being added or removed.
- Whether a moderator who isn't the broadcaster shows up the same way in `banned_by`, `unbanned_by` and `pinnedBy`.
- How kick-mod learns who deleted a message, unpinned, cleared chat or changed settings, since the events don't say. Perhaps only for its own actions.
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
