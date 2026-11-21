# kick-mod

A chat client and moderation tools for Kick channels. Kick only, built on [kick-api](kick-api.md). See the [overall rules](README.md).

## Scope (first version)
- Several Kick channels managed at once, as broadcaster on my own channels and as moderator on others.
- **Manual actions:** ban, timeout, unban and delete message, from the chat view.
- **Automod rules:** each rule says what a match does: flag it for review, delete, timeout or ban.
- **User history lookup:** a user's past messages, and the actions taken on them.
- **Mod log:** every action taken through the tool, with its reason.

## Decisions
- **Interface:** a local web UI: axum serving plain-JS pages with no build step.
- **Users:** for now one user (me), with one Kick OAuth token, on the local PC. It may later be hosted remotely and opened to other moderators, so this shouldn't be designed out.
- **Connections:** one Centrifugo connection per channel.
- **History:** kick-mod journals the chat it receives, locally, in the logger's journal format (see [logger](logger.md)).
  - A separate always-on Kick logger, like dgg's, comes later. It and kick-mod write compatible journals, so kick-mod's files can be merged into it.
  - Until then, history covers only the times kick-mod was running.
- **Config:** a TOML file, `data/kick-mod/config.toml` by default (`--config` for another), with a table per channel:
  ```toml
  [channels.<slug>]
  chatroom_id = 123
  broadcaster_user_id = 456
  ```
  - `kick-mod ids` prints these tables, so the ids are looked up once, with curl, and startup doesn't depend on Cloudflare.
  - With several moderators, some settings become theirs, such as which channels they see. These will go in a per-moderator part of the config; for now there's one moderator, who sees every channel.
- **Pushing to the page:** server-sent events (SSE). The page sends actions with plain POSTs.
- **Access:** the server listens on `127.0.0.1:8787` only, with no login yet. It refuses requests whose `Host` or `Origin` isn't its own, so other websites in the browser can't act through it.
  - Later, a login: needed before it's hosted for other moderators. Most likely each moderator signs in with Kick, which also says who they are.
- **Token:** saved in `data/kick-mod/token.json`, and saved again on each refresh.

## Steps
Each is a pause for review.
1. **Server:** `kick-mod serve`, reading the config and serving one plain page, with the `Host`/`Origin` check. `ids` prints config tables.
2. **Sign-in:** a "Sign in" button on the page goes to Kick, which redirects back to the server's own `/oauth/callback`. Later starts use the saved token. When it can't be refreshed, the page asks to sign in again.
3. **Live chat:** a realtime connection per channel, and a column per channel on the page, fed over SSE: messages, with deletions, bans and timeouts shown as they happen.
4. **Manual actions:** delete, timeout (preset durations), ban and unban on each message, with an optional reason. Each is appended to the mod log, `data/kick-mod/modlog.jsonl`: when, by whom, what, on whom, why, and Kick's answer. A failure shows Kick's error.

Later, each its own step:
- The Kick journal variant in `logger-journal`, so chat is kept.
- User history lookup, which reads it.
- Automod rules.
- The login, and per-moderator config.
