# kick-mod

A chat client and moderation tools for Kick channels. Kick only, built on
[kick-api](kick-api.md). See the [overall rules](README.md).

## Scope (first version)
- Several Kick channels managed at once, as broadcaster on my own channels and
  as moderator on others.
- **Manual actions:** ban, timeout, unban and delete message, from the chat
  view.
- **Automod rules:** each rule says what a match does: flag it for review,
  delete, timeout or ban.
- **User history lookup:** a user's past messages, and the actions taken on
  them.
- **Mod log:** every action taken through the tool, with its reason.

## Decisions
- **Interface:** a local web UI: axum serving plain-JS pages with no build step.
- **Users:** for now one user (me), with one Kick OAuth token, on the local PC.
  It may later be hosted remotely and opened to other moderators, so this
  shouldn't be designed out.
- **Connections:** one Centrifugo connection per channel.
- **History:** kick-mod journals the chat it receives, locally, in the logger's
  journal format (see [logger](logger.md)): Kick's variant, under
  `data/kick-mod/journal/<slug>/` (`--journal` for another root), with an
  instance per run, `kick-mod-<start unix ms>`.
  - A separate always-on Kick logger, like dgg's, comes later. It and kick-mod
    write compatible journals, so kick-mod's files can be merged into it.
  - Until then, history covers only the times kick-mod was running.
- **Files:** settings in a config file, and what kick-mod learns or keeps in
  `data/kick-mod/`.
  - **Config:** `kick-mod.toml` (`--config` for another), gitignored since it
    holds the app's secret. Only edited by hand; kick-mod never writes it. It
    replaces kick-mod's use of `.env`.
    ```toml
    [app]
    client_id = "..."
    client_secret = "..."

    [server]
    listen = "127.0.0.1:8787"
    ```
  - **Channels:** `data/kick-mod/channels.json`, an object per channel by slug,
    written by kick-mod and editable by hand:
    ```json
    {
      "<slug>": { "broadcaster_user_id": 456, "chatroom_id": 123 }
    }
    ```
    - kick-mod adds a channel when asked, by `kick-mod ids <slug>` now and from
      the page later. The unofficial lookup (curl) has both ids. If it fails,
      `broadcaster_user_id` comes from the official API, with the app's token,
      and the chatroom id is left for entering by hand.
    - Written whole, pretty-printed and sorted by slug. It's re-read just before
      each write, so hand edits made while kick-mod runs survive, and fields
      kick-mod doesn't know are kept.
    - It's still a race: a hand edit saved between kick-mod's read and its write
      is lost. Rare while it's one moderator; it could be handled later, by
      writing only if the file hasn't changed since the read.
      - Written to a temporary file beside it, then renamed over it. Written in
        place, a reader (an editor, or another process) could see it empty or
        half-written; renamed, it sees the old file or the new one. On Windows
        the rename can fail while another process has the file open, so a failed
        write is reported and retried, not lost.
  - **Token:** `data/kick-mod/token.json`, saved again on each refresh.
  - **Mod log:** `data/kick-mod/modlog.jsonl`.
  - With several moderators, some settings become theirs, such as which channels
    they see. They'll go in a per-moderator part of `data/kick-mod/`, since
    moderators will change them from the page; for now there's one moderator,
    who sees every channel.
- **Pushing to the page:** server-sent events (SSE). The page sends actions with
  plain POSTs.
- **Access:** the server listens on `127.0.0.1:8787` only, with no login yet. It
  refuses requests whose `Host` or `Origin` isn't its own, so other websites in
  the browser can't act through it.
  - Later, a login: needed before it's hosted for other moderators. Most likely
    each moderator signs in with Kick, which also says who they are.

## Steps
Each is a pause for review.
1. **Server:** `kick-mod serve`, reading the config and the channels and serving
   one plain page, with the `Host`/`Origin` check. `ids` records channels' ids
   in `channels.json`.
2. **Sign-in:** a "Sign in" button on the page goes to Kick, which redirects
   back to the server's own `/oauth/callback`. Later starts use the saved token.
   When it can't be refreshed, the page asks to sign in again. Signing out
   revokes the token with Kick and deletes it. The redirect URI is
   `http://localhost:<port>/oauth/callback`, so the app has to register it
   with the port kick-mod serves on.
3. **Live chat:** a realtime connection per channel, and a column per channel on
   the page, fed over SSE: messages, with deletions, bans and timeouts shown as
   they happen.
   - The channels are read when kick-mod starts: one added later is listened
     to after a restart, until channels can be added from the page.
   - A page that connects gets each channel's status, then what happens
     after. Nothing is replayed, to a page that reconnects or after Kick's
     connection drops: the page says so, and that chat is missing.
4. **Manual actions:** delete, timeout (preset durations), ban and unban on each
   message, with an optional reason. Each is appended to the mod log: when, by
   whom, what, on whom, why, and Kick's answer. A failure shows Kick's error.
   - The buttons show on hovering a message: delete, timeouts of 1m, 10m, 1h
     and 1d, and ban. A ban or timeout's line has unban. One reason box, in
     the header, goes with the next action and is cleared once it's done.
   - What's done shows when Kick's event for it arrives in the chat; only a
     refusal is shown at once.
   - A refused action doesn't sign the moderator out, even with a 400 or
     401: Kick answers a bad request with 400 too. The page's next check of
     who's signed in does.
5. **Journal:** each channel's connection is journaled as it arrives, raw:
   frames, and opens and closes.
   - A stop, which has no disconnect of its own, closes the open connection.
   - A channel whose slug can't be a directory's name isn't journaled. A
     write that fails stops that channel's journal until a restart, said on
     stderr.
   - Quitting kick-mod writes no close: the file just ends.
6. **User history lookup:** clicking a username in a channel's column opens a
   panel with that user's history in that channel, by their user id (it stays
   when they rename). Oldest first, one timeline:
   - From the channel's journal (its live files, and archives if any): their
     messages, those later deleted marked so (by Kick's AI moderation, too),
     and the bans, timeouts and unbans on them, with by whom. Frames are read
     into events by the same code as live chat.
   - From the mod log: the actions taken on them in that channel through
     kick-mod, refused ones too, with the reason and Kick's answer.
   - `GET /api/history?channel=<slug>&user=<id>`: the messages, capped at the
     newest 500, the events and the actions. The name shown is the newest seen.
   - Each request reads the files again, with no index: kick-mod's journals
     are small, only what it heard while running. The file being written is
     read up to its last complete record. An index can come if it's slow.
   - The panel shows what was there when it opened; it isn't updated live.
     It has a close button.
   - Not yet: looking up a typed name, across channels.
7. **Filling gaps:** when a channel's connection comes back after a blip,
   kick-mod asks Kick's chat history (`web.kick.com`, see
   [kick-api](kick-api.md)) for the messages sent while it was down.
   - Only blips for now: a reconnect within a run. Not the gap between runs
     (a restart, or a handover), which may come later.
   - **How far:** from the newest page back, until a page reaches the last
     thing received before the drop, with a few seconds' margin, since Kick's
     times are whole seconds and clocks differ. For a typical blip of a
     second or two, that's the first page. At most 10 pages (250 messages);
     past that, the rest of the gap stays missing.
   - Messages already received (by id) are skipped, the overlap at either
     end of the gap.
   - Only messages come back: the history has no deletions, bans, timeouts or
     unbans, so those during the gap stay missing. Whether it leaves out
     messages deleted meanwhile is unknown.
   - **Journal:** each page as received, raw, in a record of a new kind,
     `history` (Kick's format `kick.0.1`; see [logger](logger.md)), on the
     connection that came back. The user history lookup reads its messages
     too, skipping ones it has (by id).
   - **Page:** the recovered messages show in the column in time order,
     marked as recovered, and the notice of the drop says how many came back,
     or that some are still missing.
   - Needs each channel's channel id, which `channels.json` doesn't have yet:
     `ids` records it as `channel_id`, from the unofficial lookup that has the
     chatroom id. A channel without one isn't filled.
   - Requests name kick-mod in their `User-Agent` (`kick-mod/<version>`).

Later, each its own step:
- Automod rules.
- The login, and per-moderator config.
