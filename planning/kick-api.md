# kick-api

Kick's APIs, kept lightweight for any tool to depend on: [kick-mod](kick-mod.md)
now, the omnichat later. Kick only, and similar in scope to `dgg-api`. See the
[overall rules](README.md).

## Scope
- Reading chat over Kick's realtime socket.
- Kick's official public API (`https://api.kick.com/public/v1`): OAuth tokens,
  channel lookup, sending chat, deleting messages, and banning, timing out and
  unbanning.
- Each part behind its own feature, as in `dgg-api`.

## Decisions
- **Read path:** Centrifugo, which kick.com's own web client has used since
  2026-10.
  - The client follows Centrifugo's open-source client protocol
    (github.com/centrifugal/centrifugo, github.com/centrifugal/protocol). Only
    Kick's configuration of it is probed.
  - Not Pusher: it's the legacy path that Kick's site no longer uses, and its
    key changes without notice (last on 2026-10-17, to `34bf7a0ff419a2a775b9` on
    cluster `us2`).
- **Second read backend:** the official webhooks, in case the socket closes off.
  Deferred.
  - They need a public HTTPS receiver with a publicly trusted certificate, which
    the mesh's private CA can't provide.
- **Lossless:** the raw client delivers every message from every channel it's
  subscribed to.
- **Writes:** only through the official public API, with a user's OAuth token.
  Built now, though the webhooks are deferred.
- **Structure:** like `dgg-api`, fetching frames and decoding them are separate,
  so a logger can journal every frame raw, unknown ones included. The typed
  protocol is derived from those frames, and if Kick changes its protocol, the
  logger keeps working and the raw journals stay intact.
  - The realtime path has three levels, each carried inside the one above. Each
    level is named in its source's own terms: level 1 in dgg-api's, level 2 in
    Centrifugo's `client.proto` (github.com/centrifugal/protocol), and level 3
    in Kick's.
  - **Level 1, the websocket** (`realtime::client`, feature `client`): lifecycle
    events plus raw frames, in the shape of dgg-api's `chat::client::Item`.
    Centrifugo adds a session above the socket (a connect handshake, a token
    that has to be refreshed, subscriptions per channel, disconnects that
    mustn't be retried), so it adds only the events those require.
    - `Connected { connection, endpoint }`: after Centrifugo's connect reply,
      not when the socket opens.
    - `Frame(Frame)`: one websocket message's text as received, kept whole, with
      its connection number, as in dgg-api.
      - It has no arrival time: the consumer stamps it, as dgg's logger does.
        Stamping it in the client, as the frame is read, would be more accurate.
        Perhaps later, in both `dgg-api` and `kick-api`.
      - A frame can hold several newline-separated replies.
      - Every frame is yielded except pings (the empty `{}`), which are answered
        internally and not journaled, as dgg-api does with `PING`. Replies to
        the client's own commands and errors are yielded too.
    - `Binary { connection, data }`: binary (protobuf) frames. A JSON client
      shouldn't get them; they're passed through as in dgg-api.
    - `Disconnected { connection, code, reason }`: the connection dropped and
      will be retried. `code` is the server's code or the websocket close code.
    - `ConnectFailed { stage, error, retry_in }`. `stage` is Kick's `connection`
      endpoint, the token, the transport or the connect reply, so "Kick changed
      its setup" can be told apart from "the network is down".
    - `Subscribed { connection, channel }`.
    - `Unsubscribed { connection, channel, code, reason, resubscribing }`: a
      refused subscribe, the server unsubscribing the channel, or the caller
      unsubscribing it (code 0).
      - The client decides whether to resubscribe, as centrifuge-js does, and
        says so in `resubscribing`. Otherwise the channel stays a gap.
      - Resubscribed: a server unsubscribe with code 2500 or up, at once; a
        subscribe refused with a `temporary` error (100 internal, 111 too many
        requests) or 109 (token expired), after a backoff with jitter.
    - `Stopped { code, reason }`: the session ended in a way centrifuge-js
      doesn't retry either. The stream ends after it. Starting a new client is
      the supervisor's call.
      - A disconnect code of 3500 or up.
      - A connect refused with an error that isn't `temporary` or 109, such as
        101 unauthorized.
      - A refresh refused with an error that isn't `temporary`. A temporary one
        is retried after 5–10 s, as is a token that couldn't be fetched, and the
        connection stays up meanwhile.
    - Channel names are opaque strings. That a chatroom is
      `chatrooms.<chatroom id>.v2` plus `channel_<channel id>` belongs in the
      tools' config.
    - The client answers pings, refreshes the token before it expires, and
      reconnects with backoff, then resubscribes.
    - It decodes frames with level 2 only to run the connection. If one doesn't
      decode, it's still yielded.
    - Details such as the client id, `ttl` and the subscribe results are left to
      level 2, in the frames.
    - `realtime::connection`: Kick's setup, with its endpoints' names:
      `connection` answers the endpoint, `auth/connection` the token. It's kept
      apart from the socket code, so a supplied endpoint and token can bypass
      it.
  - **Level 2, Centrifugo's client protocol** (`realtime::centrifugo`, feature
    `centrifugo`): the types and names of `client.proto`. Messages are structs
    and one-ofs are enums.
    - A `Frame` decodes into `Reply`s: `id`, `error`
      (`Error { code, message, temporary }`), and one of the results (`connect`,
      `subscribe`, `refresh`, `unsubscribe`, `history`, …) or a `push`. An empty
      `Reply` is a ping.
    - `Push`: `channel`, and one of `pub` (`Publication`), `join`, `leave`,
      `unsubscribe`, `message`, `subscribe`, `connect`, `disconnect` or
      `refresh`.
    - `Publication`: `data` (raw JSON here), `info`, `offset`, `tags`, ….
    - `Command` with the requests the client sends (`ConnectRequest`,
      `SubscribeRequest`, `RefreshRequest`).
    - A feature of its own, so a journal reader can decode frames without tokio.
      `client` depends on it.
  - **Level 3, Kick's events** (`realtime::event`, feature `protocol`): a
    `Publication`'s `data`, `{event, data}`, with `data` a JSON string, decoded
    into `Event`.
    - Variants take Kick's names without the `App\Events\` prefix:
      `ChatMessageEvent`, `MessageDeletedEvent`, `UserBannedEvent`,
      `UserUnbannedEvent`, `PinnedMessageCreatedEvent`,
      `PinnedMessageDeletedEvent`, `ChatroomUpdatedEvent`, `ChatroomClearEvent`,
      `ChatSettingsChanged`.
    - `Other { event, data }` for an event not typed yet, and
      `Unreadable { event, data, error }` for a known one whose data doesn't
      decode, as in `dgg-api`.
    - Fields take Kick's JSON names, in snake_case where Kick isn't (`pinnedBy`,
      `aiModerated`).
    - `metadata.message_ref` is an opaque string. Kick's web client happens to
      put a timestamp there, but any sender can put anything, so nothing reads
      it as a time or checks whether it looks like one.
    - Names are matched whole, prefix included. A typed event sent under another
      name, such as `ChatMessageEvent` without its prefix, is `Other`.
    - Required (built 2026-11-20):
      - a message's `id`, `content` and `sender`;
      - a user's `id` and `username`;
      - a deleted message's `id`;
      - a ban's `user` and `permanent`, and an unban's `user`;
      - a pinned message;
      - `ChatroomUpdatedEvent`'s `id`, `ChatSettingsChanged`'s `settings`, a
        channel's `id`;
      - each setting's `enabled`.
    - `PinnedMessageDeletedEvent`'s `[]` decodes to an empty event. An object
      decodes too, with its fields in `rest`.
    - Times (`created_at`, `expires_at`) are unix milliseconds, as in `dgg-api`,
      parsed from Kick's RFC 3339 strings (`+00:00` or `Z`). The offset and
      spelling aren't kept, but the raw frame has them. A string that doesn't
      parse stays in `rest`.
  - **Decoding, levels 2 and 3:**
    - Only the fields an object needs to mean anything are required. Everything
      else is optional, so a change degrades the one field instead of the whole
      event.
    - Fields an object doesn't name aren't dropped. Each struct keeps them in
      `rest: Option<Map>`, which is `None` when nothing was left over, so a
      field Kick adds or renames moves into `rest` rather than being lost.
    - `Event::complete()` says whether the event is well-formed by the known
      rules and nothing unknown is left over: it's a typed variant (not `Other`
      or `Unreadable`) and every nested `rest` is `None`.
      - It doesn't mean every field is filled in. An optional field that's
        absent is still complete.
    - `dgg-api` drops unnamed fields from its typed events. It stays that way
      for now: its flattened structs make leftovers fiddly to place.
  - `official`: the official API over reqwest: OAuth (app and user tokens, and
    refreshing them), channel lookup, sending chat, deleting, banning, timing
    out and unbanning.
    - `official::oauth`: Kick's sign-in, as data and requests, with no server of
      its own.
      - It builds the authorize URL from the client id, redirect URI and scopes,
        with a random `state` and a PKCE verifier (`S256`), and returns those
        for the caller to keep until the callback.
      - It exchanges the code for a user token, refreshes one, revokes one, and
        fetches app tokens.
      - A `Token` holds the access and refresh tokens, the scopes, and when it
        expires, in unix milliseconds.
      - The callback on `http://localhost:8787/oauth/callback` belongs to the
        tool: kick-mod serves it from its own web server. An example shows the
        whole flow with a one-shot listener.
    - `official::Api`: one method per endpoint: users, channels, sending chat,
      deleting a message, banning or timing out, and unbanning.
      - Responses are typed, and fields they don't name are kept in `rest`, as
        in the realtime events.
      - An error keeps the HTTP status and Kick's `message`, and tells apart a
        429 (wait) and a 403 that looks like the Cloudflare block.
      - Base URLs can be set, so the tests run against a local fake server, as
        the realtime client's do.
    - **Refreshing the token:** `Api` isn't a service. It has no task and no
      timer, and does nothing between calls.
      - `Api::refresh()` refreshes now, however long the token has left. It's
        the only path to a new token: everything below goes through it.
        - A caller can use it to keep the token alive without other calls, or at
          startup to check that a saved token still works. This helps only if
          Kick expires refresh tokens that go unused, which is unknown.
      - `Api::token()` is the current `Token`, so the caller can see
        `expires_at` and choose when to refresh.
      - A call checks the user token first. If it expires within a minute, the
        call goes through `refresh()` before sending the request. A 401 goes
        through `refresh()` once and retries.
      - Concurrent calls share one refresh, whether a call or the caller started
        it.
      - Kick answers each refresh with a new refresh token, so `refresh()` hands
        each new `Token` to a callback the caller supplies, which saves it.
        Whether the old one still works isn't documented, so it's always
        replaced.
      - A refresh that fails is returned as the error of the call that asked for
        it, and isn't retried. The tool then asks the user to sign in again.
      - With an app token, `refresh()` fetches a new app token, and there's no
        callback, since there's nothing to keep.
      - `oauth::App`'s token requests stay the low-level steps: `Api` calls them
        only from `refresh()`, and a tool calls them only to sign in.
      - The realtime client's token is Centrifugo's, from Kick's realtime setup,
        and not an OAuth token. It's refreshed on its own, as now.
  - `channel`: decoding the unofficial channel lookup, as now.

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
  - Cloudflare refuses reqwest there with
    `{"error": "Request blocked by security policy."}`, but lets curl through,
    so it's probably blocking on the TLS fingerprint.

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
- For the webhooks: whether an app token may subscribe to any channel's
  webhooks.

## The webhook probe
It lives on the `kick` branch (`viewer/examples/kick.rs`, with `.env.example`).
It compiles but hasn't been run.
- **Running it needs:**
  - a Kick app with webhooks turned on;
  - a tunnel such as `cloudflared tunnel --url http://localhost:8787`;
  - `.env` filled in from `.env.example`.

  Then, on the `kick` branch, run
  `cargo run -p viewer --example kick -- <slug>`.
