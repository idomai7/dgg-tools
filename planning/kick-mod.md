# kick-mod

A chat client and moderation tools for Kick channels. Kick only, built on [kick-api](kick-api.md). See the [overall rules](README.md).

## Scope (first version)
- Several Kick channels managed at once.
- **Manual actions:** ban, timeout, unban and delete message, from the chat view.
- **Automod rules.**
- **User history lookup:** a user's past messages, and the actions taken on them.
- **Mod log:** every action taken through the tool, with its reason.

## Decisions
- **Interface:** a local web UI: axum serving plain-JS pages with no build step.

## Open questions
- Where user history comes from. Kick's API has no chat history, so the tool has to log chat itself. Possibly into a journal like the logger's, and only for as long as it's running?
- **Automod:** does it act on its own or flag messages for review, and what's the rule format?
- **Mod log:** local only, or shared between mods?
- **Accounts:** one OAuth user token per mod? Is the user the broadcaster or a moderator on these channels?
- **Where it runs:** only on the PC, or also reachable from elsewhere (a phone, the droplet), which would need its own login?
