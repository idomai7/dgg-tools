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
