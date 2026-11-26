# kick-api

Kick's APIs, kept lightweight for any tool to depend on: the logger and
[kick-mod](kick-mod.md) now, the omnichat later. See the
[overall rules](README.md). Each part is behind its own feature, as in
`dgg-api`; the types' rustdoc has the details.

## Scope
- Kick's realtime socket, for reading chat.
- Kick's official public API (`https://api.kick.com/public/v1`): OAuth
  tokens, channel lookup, sending chat, deleting messages, and banning,
  timing out and unbanning.
- `web.kick.com`'s unofficial API: a channel's past chat.
- The unofficial channel lookup, for channel and chatroom ids.

## Decisions
- **Read path:** Centrifugo, which kick.com's own web client has used since
  2026-10, following Centrifugo's open-source client protocol
  (github.com/centrifugal/centrifugo, github.com/centrifugal/protocol);
  only Kick's configuration of it was probed. Not Pusher: the legacy path
  Kick's site no longer uses, whose key changes without notice.
- **Webhooks, deferred:** a second read path in case the socket closes
  off. They need a public HTTPS receiver with a publicly trusted
  certificate, which the mesh's CA can't give.
- **Writes** go only through the official API, with a user's OAuth token.
- **Raw first:** as in `dgg-api`, fetching frames and decoding them are
  apart, so the logger journals every frame raw, unknown ones included,
  and keeps working if Kick changes its protocol. The realtime path has
  three levels, each named in its source's terms:
  1. **The websocket** (`realtime::client`, feature `client`): lifecycle
     events and raw frames, shaped like `dgg-api`'s, plus what
     Centrifugo's session needs: `Connected` (after the connect reply),
     `Frame`, `Binary`, `Disconnected` (retried), `ConnectFailed { stage }`
     (Kick's setup, the token, the transport or the reply, so "Kick
     changed" is told from "the network is down"), `Subscribed`,
     `Unsubscribed { resubscribing }` and `Stopped` (ended in a way
     centrifuge-js doesn't retry either: a disconnect code of 3500 or up,
     or a connect or refresh refused with a non-`temporary` error).
     - It answers pings (not yielded), refreshes the token before it
       expires (a temporary failure retried after 5 to 10 s), reconnects
       with backoff and resubscribes. It resubscribes as centrifuge-js
       does: a server unsubscribe of 2500 or up at once, a `temporary`
       refusal or 109 after a backoff with jitter.
     - Frames carry no arrival time: the consumer stamps them.
     - Channel names are opaque; `chatrooms.<id>.v2` and `channel_<id>`
       are the tools' business. Kick's setup (`realtime::connection`) is
       apart, so a supplied endpoint and token can bypass it.
  2. **Centrifugo's client protocol** (`realtime::centrifugo`): the types
     and names of `client.proto`; `Reply`, `Push`, `Publication`,
     `Command`. Its own feature, so a journal reader needs no tokio.
  3. **Kick's events** (`realtime::event`, feature `protocol`): a
     publication's `{event, data}` into `Event`, variants named as Kick's
     without the `App\Events\` prefix (matched whole, prefix included),
     plus `Other` and `Unreadable` as in `dgg-api`. Times are unix
     milliseconds from Kick's RFC 3339 strings.
     `metadata.message_ref` is opaque: any sender can put anything there.
- **Decoding:** only the fields an object needs to mean anything are
  required, so a change degrades one field, not the event. Fields an
  object doesn't name are kept in `rest` (`None` when nothing's left
  over), so a field Kick adds moves there rather than being lost.
  `Event::complete()` says it's typed and nothing is left over. (`dgg-api`
  still drops unnamed fields: its flattened structs make that fiddly.)
- **Official API** (`official`): `official::oauth` is Kick's sign-in as
  data and requests (authorize URL with `state` and PKCE `S256`, code
  exchange, refresh, revoke, app tokens), with no server: the callback is
  the tool's. `official::Api` has a method per endpoint, typed answers
  keeping `rest`, and errors keeping the status and Kick's `message`,
  telling apart a 429 and a 403 that looks like Cloudflare's block.
  - **Tokens:** `Api` is not a service: no task, no timer. A call
    refreshes first if the token expires within a minute, and once on a
    401; concurrent calls share one refresh; `Api::refresh()` is the only
    path to a new token, handing each to the caller's callback to save,
    since Kick answers every refresh with a new refresh token. A failed
    refresh is the call's error, not retried: the tool asks the user to
    sign in again.
- **Web API** (`web`): `Web::new(http, user_agent)`, every request naming
  the tool honestly, never as a browser. `history(channel_id,
  cursor)` gives a page newest first and the cursor before it;
  `cursor_at(ms)` starts at a time. `Web::back_to` pages back to a time,
  for filling gaps. No retries or pacing: one page at a time.
- **Fakes:** every client's base URL can be set, so tests run against a
  local fake server.
- **Kick's terms (read 2026-11-24):** the Terms of Service (§1.4) forbid
  automated access, access by means it hasn't provided, and bypassing its
  access measures; the developer terms (§10.1) forbid undocumented
  endpoints and limit keeping chat logs to what the tool needs. The
  realtime socket, the channel lookup and the chat history are all
  undocumented. Used anyway, knowingly: read-only, low volume, for
  moderating our own channels, with an honest `User-Agent`. The cost: any
  may break or be blocked without notice, and the app's credentials may
  be at risk. Hence the logger reads them anonymously, and kick-mod,
  holding the app's credentials, never does.

## Known
### Centrifugo (probed 2026-11-19)
Two minutes on a busy live channel, plus an offline one (`destiny`), from a
scratch probe. Then a scripted run on a quiet channel of our own, where its
broadcaster sent messages, deleted, pinned, timed out, banned, unbanned, toggled
each chat setting and cleared chat.
- **Connecting:** no login, and no `Origin` header is needed anywhere. reqwest
  is accepted on `web.kick.com`.
  1. `POST https://web.kick.com/api/v1/realtime/connection` with:
     ```js
     {
       "client": {"id": <uuid>, "type": "web"},
       "capabilities": {"accepted_providers": [{"provider": "centrifugo"}]}
     }
     ```
     It answers:
     ```js
     {
       data: {
         connections: [{provider: "centrifugo", credentials: {url}}],
         mode: "websocket"
       },
       message: "success"
     }
     ```
     The `url` was
     `wss://realtime.us-west-2.platform.kick.com/connection/websocket` on the
     first run and `us-east-1` on the second, so it changes between connections
     and has to be asked for each time.
  2. `POST https://web.kick.com/api/v1/realtime/auth/connection` with
     `{"client_id": <the same uuid>}`. It answers `{data: {token}}`, an HS256
     JWT for `sub: guest:<uuid>` that lasts 45 minutes.
  3. Open the socket and send:
     ```js
     {"id": 1, "connect": {"token": <token>, "name": "js"}}
     ```
     The reply was:
     ```js
     {
       client,
       version: "6.9.7 PRO",
       expires: true,
       ttl: 2699,
       ping: 25,
       pong: true
     }
     ```
- **Server:** Centrifugo PRO, the commercial build. It speaks the open-source
  JSON protocol.
- **Keepalive:** the server sends `{}` every 25 s, and the client answers each
  with `{}`.
- **Token refresh:** `expires: true` with a `ttl`, so the token has to be
  refreshed before it runs out.
- **Channels:** subscribing to `chatrooms.<chatroom id>.v2` answers
  `{"subscribe": {}}`. It carries everything below except `ChatSettingsChanged`.
  - `channel_<channel id>` carries `ChatSettingsChanged`.
  - `chatrooms.<id>`, `chatroom_<id>` and `channel.<channel id>` were accepted
    too, but carried nothing in either run. Whether they're real channels or the
    server accepts any name is unknown.
  - Reportedly at most 128 subscriptions per connection (error 106 beyond that).
- **No history or recovery:** the subscribe reply has no `recoverable`, `offset`
  or `epoch`, and `history` answers `108 not available`. Whatever arrives while
  disconnected is lost.
- **Frames:** one push per frame:
  ```js
  {
    "push": {
      "channel",
      "pub": {
        "data": {"event", "data": "<JSON as a string>"},
        "tags": {"event"}
      }
    }
  }
  ```
- **Events seen:**
  - `App\Events\ChatMessageEvent`:
    - `id` (uuid), `chatroom_id`, `content`, `type` (`message` or `reply`),
      `created_at`, `metadata.message_ref`.
    - `sender`: `id`, `username`, `slug`, and `identity` (`color`, `badges` with
      `type` such as `broadcaster`, `moderator`, `subscriber`, `verified` or
      `founder`, plus `badges_v2`).
    - `created_at` is whole seconds only, set by the server.
    - `message_ref` is set by the sender's client, not the server. Kick's web
      client (its bundle, read 2026-11-20) sends
      `message_ref: String(Date.now())` with each message, reply and emote
      click, and the server echoes it back in `metadata`. The client uses it to
      match the echo to the message it showed optimistically.
      - So it's the sender's clock, in milliseconds since the epoch (no
        timezone), about 0.5–1.2 s before the push arrives. A sender can put
        anything there, and other clients may leave it out.
      - Tested on our own channel through the web API (2026-11-20): Kick's
        server takes any string of up to 36 characters (a uuid's length) and
        echoes it unchanged. That includes `""`, `"not-a-time"` and
        `"9999999999999"`. A number is echoed as a string, and a longer string
        is refused with a 422. Leaving it out is accepted, and then the echoed
        `metadata` is `null`. A repeated value is accepted too, so it isn't used
        to drop duplicates.
    - Emotes are inline in `content` as `[emote:<id>:<name>]`. Unicode and emoji
      arrive as-is, and a long message (about 450 characters) arrives whole.
    - The broadcaster's link went through with `allow_links` off.
  - `App\Events\MessageDeletedEvent`: `id`, `message.id`, and for Kick's AI
    moderation `aiModerated: true` and `violatedRules` (e.g. `hate`). Nothing
    says who deleted the message, even when the broadcaster did.
  - `App\Events\UserBannedEvent`: `id`, `user` (`id`, `username`, `slug`),
    `banned_by`, `permanent`.
    - A timeout is `permanent: false` with `duration` in minutes and
      `expires_at`. A ban is `permanent: true` with neither.
    - `banned_by` names the moderator by `username` and `slug`, but its `id` is
      always 0, even for the broadcaster's own bans.
  - `App\Events\UserUnbannedEvent`: `id`, `user`, `unbanned_by` (with its real
    `id`), and `permanent`, which says whether it lifted a ban or a timeout.
    Removing a timeout is an unban.
  - `App\Events\PinnedMessageCreatedEvent`: the whole `message` (as in
    `ChatMessageEvent`, plus `thread_parent_id`), `duration` in seconds as a
    string (`"1200"`), and `pinnedBy` with full identity.
  - `App\Events\PinnedMessageDeletedEvent`: data `[]`. Nothing says which
    message or who unpinned it.
  - `App\Events\ChatroomClearEvent`: only an `id`. Nothing says who cleared it.
  - `App\Events\ChatroomUpdatedEvent` and `ChatSettingsChanged` (on
    `channel_<id>`, without the `App\Events\` prefix): every settings change
    sends both, within 0.1 s, in either order. Each carries all the settings,
    not just the one that changed, and neither says who changed them.
    - `ChatroomUpdatedEvent`: `id` (the chatroom), `slow_mode` (`enabled`,
      `message_interval` in seconds), `followers_mode` and `account_age`
      (`enabled`, `min_duration` in minutes), `subscribers_mode`, `emotes_mode`.
    - `ChatSettingsChanged`: `id` (uuid), `channel` (`id`, `slug`),
      `created_at`, and `settings` with every duration in seconds, plus `rules`
      and `allow_links`, which `ChatroomUpdatedEvent` lacks.
- **Chatroom id:** different from the channel id and the user id (for `destiny`:
  chatroom 1764849, channel 1772249, user 1823122).
  - It's on the unofficial `GET https://kick.com/api/v2/channels/<slug>`
    (`chatroom.id`).
  - Cloudflare refused reqwest there with
    `{"error": "Request blocked by security policy."}`, but let curl through,
    so it was taken for blocking on the TLS fingerprint. More likely it was
    the `User-Agent`, as on `web.kick.com` (below): so since 2026-11-25 the
    lookup asks with reqwest, naming the tool by its codename.

### Chat history (probed 2026-11-24)
Found in kick.com's own page scripts, then asked anonymously for `destiny`.
- `GET https://web.kick.com/api/v1/chat/<channel id>/history`: no sign-in. It
  answers `{data: {messages, cursor, pinned_message}, message: "OK"}`, 25
  messages a page, newest first.
  - It takes the channel id (`destiny`: 1772249), which only the unofficial
    channel lookup has; each message's `chat_id` is it too.
  - A message: `id`, `chat_id`, `user_id`, `content`, `type` (`message` or
    `reply`), `metadata` (a JSON string, not an object as in the realtime
    events), `sender` (`id`, `slug`, `username`, `identity` with `color`,
    `badges` and `badges_v2`), `created_at` (RFC 3339) and
    `thread_parent_id`.
  - `cursor` is a unix time in microseconds: `?cursor=<µs>` answers the
    messages before it, and its own `cursor` is the page before that. It
    reached back a year (2025-11); 2024-01 answered nothing.
  - `?start_time=<RFC 3339>` is for replaying a past stream's chat; it
    answered no messages as tried.
  - Whether deleted messages, or banned users', are left out is unknown.
- reqwest is refused (403 `Request blocked by security policy.`) without a
  `User-Agent` or with curl's, but let through with `kick-mod`.
- Also in the page's scripts, not probed: on `web.kick.com`,
  `/api/v1/channels/{id}/messages-by-thread/{thread}` (with `cursor`) and
  `/api/v1/channels/{id}/chat/active-chatters`; and on `kick.com`, signed in
  with the site's session, `/api/v2/channels/<channel id>/users/<user
  id>/messages` (with `cursor`), the moderator's user card.

### Official API
- App token via `POST https://id.kick.com/oauth/token` (`client_credentials`).
- User sign-in (docs read 2026-11-21): `GET https://id.kick.com/oauth/authorize`
  with `response_type=code`, `client_id`, `redirect_uri`, `scope`, `state`
  (required) and `code_challenge` with `code_challenge_method=S256` (the only
  method).
  - The code goes to `POST https://id.kick.com/oauth/token` (form) with
    `grant_type=authorization_code`, `client_id`, `client_secret`,
    `redirect_uri`, `code_verifier` and `code`. `grant_type=refresh_token` takes
    `refresh_token` instead.
  - Both answer `access_token`, `token_type`, `refresh_token`, `expires_in` and
    `scope`.
  - `POST https://id.kick.com/oauth/revoke` with `token` and `token_hint_type`
    (`access_token` or `refresh_token`) as query parameters.
  - Scopes kick-mod needs: `user:read`, `channel:read`, `chat:write`,
    `moderation:ban`, `moderation:chat_message:manage`.
- Every response is `{data, message}`.
- Channel lookup via `GET /public/v1/channels?slug=` or `?broadcaster_user_id=`,
  up to 50 of one or the other (both is a 400). It gives `broadcaster_user_id`,
  `slug`, `stream` and more, but no channel or chatroom id. With no parameters,
  it's the signed-in user's channel.
- `GET /public/v1/users?id=`: `user_id`, `name`, `email`, `profile_picture`.
  With no `id`, it's the signed-in user.
- `POST /public/v1/chat` (`chat:write`): `content` (at most 500 graphemes and
  2048 bytes of UTF-8), `type` (`user` or `bot`), `broadcaster_user_id` (needed
  as a user, ignored as a bot), and `reply_to_message_id`. It answers `is_sent`
  and `message_id`.
- `DELETE /public/v1/chat/{message_id}` (`moderation:chat_message:manage`): 204.
- `POST /public/v1/moderation/bans` (`moderation:ban`): `broadcaster_user_id`,
  `user_id`, `duration` in minutes (1 to 10080, omitted for a permanent ban) and
  `reason` (at most 100 characters).
- `DELETE /public/v1/moderation/bans` (`moderation:ban`): `broadcaster_user_id`
  and `user_id`. It lifts a ban or a timeout.
- Webhooks: subscribe with `POST /public/v1/events/subscriptions` (e.g.
  `chat.message.sent`). The webhook URL is set in the app settings.
- Webhook verification: RSA PKCS#1 v1.5 SHA-256 over `messageId.timestamp.body`,
  using the `Kick-Event-*` headers. The public key comes from
  `/public/v1/public-key`.
- Webhook payload: `content`, with emotes inline as `[emote:ID:NAME]`, plus
  `sender.identity` and `created_at`.
- Cloudflare has reportedly refused some clients, with 403s on
  `POST /public/v1/chat` and on channel lookups (KickDevDocs issues #281, #434).
- Checked live with the sign-in example (2026-11-21), over reqwest and rustls,
  which weren't refused:
  - Signing in, `GET /users`, `GET /channels`, `POST /chat` and `DELETE /chat`
    all worked.
  - `POST /chat`'s `message_id` is the realtime message's `id`, and
    `DELETE /chat` takes it; the deletion arrives with the same id.
  - The realtime sender's `id` is the official API's `user_id`. Assumed, not
    checked, to be the `user_id` the bans take.
  - Kick grants only the scopes enabled in the app's settings: `moderation:ban`
    was asked for but missing until it was enabled there.
  - A saved token past its expiry was refreshed, and the new one kept.
- Sending as a bot, tried with the bot example (2026-11-22):
  - With the app's token: 401 `Unauthorized`. Only a user's token sends chat.
  - With the user's token: 404 `Not found`. Perhaps because the app has no bot
    yet: others report one is created from the app's settings (KickDevDocs
    #343).

## Open questions
- The other events: subscriptions and gifts, going live and offline, title and
  category changes, follows, replies (seen as `type: reply` but not looked at),
  polls, and moderators being added or removed.
- Whether a moderator who isn't the broadcaster shows up the same way in
  `banned_by`, `unbanned_by` and `pinnedBy`.
- How kick-mod learns who deleted a message, unpinned, cleared chat or changed
  settings, since the events don't say. Perhaps only for its own actions.
- Where to get a chatroom id from a slug without the unofficial lookup. Perhaps
  the official channel lookup has it; otherwise the user enters it once and the
  tool keeps it.
- How long a refresh token lasts when it isn't used.
- Whether chat history leaves out deleted messages or banned users'.
- For the webhooks: whether an app token may subscribe to any channel's
  webhooks.

## The webhook probe
It lives in commit `58ed214`, the tip of the old `kick` branch (deleted
2026-11-26; the commit is in `main`'s history): `viewer/examples/kick.rs`, with
`.env.example`. It compiles but hasn't been run.
- **Running it needs:**
  - a Kick app with webhooks turned on;
  - a tunnel such as `cloudflared tunnel --url http://localhost:8787`;
  - `.env` filled in from `.env.example`.

  Then, with that commit checked out, run
  `cargo run -p viewer --example kick -- <slug>`.
