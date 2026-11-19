# kick-api

Kick's APIs. Deferred. See the [overall rules](README.md).

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
