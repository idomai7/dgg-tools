# kick-mod

A chat client and moderation tools for Kick channels, codenamed gavelgab,
built on [kick-api](kick-api.md) and the [logger](logger.md). See the
[overall rules](README.md).

## Scope (first version)
- Several Kick channels at once: as broadcaster on my own, as moderator on
  others.
- **Manual actions:** ban, timeout, unban and delete message, from the chat.
- **Automod rules:** each says what a match does: flag it for review,
  delete, timeout or ban.
- **User history:** a user's past messages in a channel, and the actions
  taken on them.
- **Mod log:** every action taken through the tool, with its reason.

## Decisions
- **Kick, only officially:** kick-mod talks to Kick only through its
  official API, signed in (`add` too, for a channel's broadcaster id).
  Live chat and history come from the logger, which reads Kick's
  undocumented endpoints anonymously, so nothing tied to the account or
  the app touches an undocumented endpoint. (Until 2026-11-25 kick-mod
  had its own Centrifugo connection, journal and gap filling; its old
  journals stay in `data/kick-mod/journal/`, in the logger's format.)
- **Interface:** a local web UI, axum serving plain-JS pages with no build
  step. Server-sent events push to the page; actions are plain POSTs.
- **Users:** one for now (me), with one Kick OAuth token, on the local PC.
  It may later be hosted for other moderators, so that isn't designed out:
  some settings (which channels they see) would become theirs, in a
  per-moderator part of `data/kick-mod/`.
- **Access:** listens on `127.0.0.1:8787` only, with no login yet. It
  refuses requests whose `Host` or `Origin` isn't its own, so other sites
  in the browser can't act through it. A login (most likely signing in
  with Kick) comes before it's hosted for others.
- **Requests to Kick** time out after 30 s.

## Files
- **Config:** `kick-mod.env.jsonc` (or `--config-file`, or
  `--config-stdin`), JSONC like every config with secrets (see the
  [overall rules](README.md)). Gitignored; edited by hand only.
  `kick-mod config` shows it with each secret as `<secret>`.

  ```jsonc
  {
    "app": {"client_id": "...", "client_secret": "..."},
    "server": {"listen": "127.0.0.1:8787"},
    "logger": {
      "url": "https://pi:8788",
      "token": "...",
      "ca": "-----BEGIN CERTIFICATE-----\n..."
    }
  }
  ```

  `server` is optional. `logger` is required: a node's `serve`, its query
  token, and the mesh CA's certificate.
- **Channels:** `data/kick-mod/channels.json`, by slug:

  ```json
  {
    "<slug>": {"broadcaster_user_id": 456}
  }
  ```

  `kick-mod add <slug>...` looks up `broadcaster_user_id` with the official
  API. Written whole, sorted, through a temp file renamed over it (a
  reader sees the old file or the new, never half of one; on Windows a
  rename can fail while the file is open, so a failed write is reported
  and retried). Re-read just before each write, so hand edits survive and
  unknown fields are kept; a hand edit saved between the read and the
  write is still lost, which is rare with one moderator.
- **Token:** `data/kick-mod/token.json`, saved again on each refresh.
- **Mod log:** `data/kick-mod/modlog.jsonl`.

## How it works
- **Sign-in:** the page's "Sign in" goes to Kick, which redirects back to
  `http://localhost:<port>/oauth/callback`, so the app must have that
  redirect URI with the port served on. Later starts use the saved token;
  when it can't be refreshed, the page asks to sign in again. Signing out
  revokes the token and deletes it.
- **Live chat:** a column per channel, from the logger's
  `/kick/v0/channels/<slug>/events`. The logger's opens and closes of its
  connection to Kick are the column's status. A stream starts live:
  nothing is replayed, and a page that reconnects is told chat is missing.
  A dropped stream is retried after 1 s, doubling to 30 s; 45 s of silence
  counts as a drop; `400`, `401` or `403` stops the channel, as the config
  needs fixing. A gap's history pages arrive as `filled` messages: those
  since following started that weren't already seen. Channels are read at
  start; one added later needs a restart.
- **Actions:** on hovering a message: delete, timeouts of 1m, 10m, 1h and
  1d, and ban; a ban's or timeout's line has unban. One reason box, in the
  header, goes with the next action. Each is appended to the mod log (when,
  by whom, what, on whom, why, and Kick's answer). What's done shows when
  Kick's event for it arrives; only a refusal shows at once, with Kick's
  error. A refusal never signs the moderator out (Kick answers a bad
  request with 400 too); the page's next check of who's signed in does. A
  delete's message id must be letters, digits and dashes.
- **User history:** `GET /api/history?channel=<slug>&user=<id>` (by user
  id, which stays when they rename): the logger's history of them in the
  channel (messages, the newest 500, with deletions; bans, timeouts and
  unbans, with by whom; the newest name), plus the mod log's actions on
  them there, refused ones too. A channel the logger doesn't have (`404`)
  gives only the mod log's; a logger that can't be asked gives `502`.

## Open
- **The page's missing parts:** the history panel (clicking a username
  opens it, oldest first, with a close button, as of when it opened; only
  `/api/history` exists), and showing `filled` messages in time order,
  marked recovered, the drop's notice saying how many came back.
- A live test against the logger, run by the user.
- Adding channels from the page; looking up a typed name across channels.
- Automod rules.
- The login, and per-moderator config.
