# dgg-api

destiny.gg's APIs, kept lightweight for any tool to depend on. See the [overall rules](README.md).

## destiny.gg chat (from chat-gui: https://github.com/destinygg/chat-gui)
- **Socket:** `wss://chat.destiny.gg/ws`. Each frame is `EVENT {json}`, split on the first space (`assets/chat/js/source.js`).
  - Answer `PING` with `PONG <data>`.
  - The handlers live in `assets/chat/js/chat.js`: MSG, NAMES, HISTORY, JOIN/QUIT, MUTE/BAN, BROADCAST, SUBSCRIPTION/GIFTSUB/MASSGIFT/DONATION, and others.
- **MSG fields:** `nick`, `data`, `timestamp`, `features` (flair names), `roles`, `subscription`, `watching`.
- **Origin:** chat-gui's dev proxy forces `Origin: https://www.destiny.gg`. Anonymous connections work with or without it (tested).
- **Emotes and flairs:** see the next section.

## destiny.gg chat assets (surveyed 2026-11-09)
Sources: what `cdn.destiny.gg` served, chat-gui (d3f4451, 2026-11-08), and the website's `Destiny\Chat\EmoteService` / `FlairService` (public repo, 2021; today's files still match it).
- **Files:** `{cdn}/emotes/emotes.json`, `emotes.css`, `{cdn}/flairs/flairs.json`, `flairs.css`.
  - chat-gui appends `?_=<cacheKey>`. The CDN caches for 4 h (`Cache-Control: max-age=14400`); `REFRESHCHATASSETS {cacheKey}` announces new files, and chat-gui refetches all four.
  - Pictures: the JSON's `image[0].url` is versioned (`/<site version>/emotes/<file>`); the CSS's `url("<file>")` resolves to `/emotes/<file>`. Both are served.
- **emotes.json:** 325 emotes, 75 KB, in the site's order (prefix, case-insensitive).
  - `prefix`, `creator` (`""` or null when uncredited), `twitch` (all false), `theme` (all 1), `minimumSubTier` (324 at 0, one at 5), `image` (always one: `url`, `name`, `mime`, `width`, `height`).
  - Mimes: PNG 290, GIF 30, AVIF 4, WebP 1.
  - chat-gui also reads `patreonOnly`, which the file doesn't have. Who can use an emote: privileged roles (ADMIN, MODERATOR, PROTECTED) any; otherwise Patreon members if `patreonOnly`, else `minimumSubTier <= ` the user's tier.
- **flairs.json:** 46 flairs, 13 KB, by priority (lowest first).
  - `name` (what a user's `features` list), `label`, `description` (null or `""` for none), `hidden` (21), `priority`, `color` (`""` in 25), `rainbowColor` (2: `flair42`, `flair33`), `image` (Head Mod's entry is all nulls).
  - Sub tiers 1 to 5: `flair13`, `flair1`, `flair3`, `flair8`, `flair42`. Every subscriber also has `subscriber`.
  - Username colour: of the user's flairs with a colour or a rainbow, the lowest priority. `hidden` only changes chat-gui's user info card (label instead of picture).
- **flairs.css (6 KB):** entirely generated from flairs.json, in reverse order, after a fixed preamble (`:root` rainbow gradient variables, `@keyframes move`).
  - Per flair: `.flair.<name>` is `display: none !important` if hidden, else the picture (`background-image`, `height`, `width`, `order: <priority>`).
  - Then `.user.<name> { color }` if it has a colour, or for a rainbow flair a gradient-text rule plus a `:hover::before` underline (newer than the 2021 code).
- **emotes.css (252 KB):** generated cores, then hand-written styles.
  - Cores (56 KB): per emote, in emotes.json's order, `.emote.<prefix> { background-image; height; width }` and `.msg-chat .emote.<prefix> { margin-top: -<height>px; top: <height/4>px }`. Indented 4 spaces now; 2 in the 2021 code.
  - Styles (196 KB): each emote's `styles` column (not in the JSON), `{PREFIX}` replaced by its prefix, appended in emote order with nothing between them. About 131 emotes have some.
  - What they hold: 292 `@keyframes`, one `@supports`, 103 comments, one inline base64 AVIF, hover effects, `::before`/`::after`, and rules across emotes: neighbours (`.emote.A + .emote.B`, `:has(+ ...)`, `:is(...)` lists), combo counts (`.msg-emote[data-combo-group="x5"]` up to `x99`), sub tiers (`.user.flair8 ~ .text .emote.X`), single users (`[data-username="..."]`) and timestamps (`time[data-unixtimestamp*="67"]`).
  - Two GIFs load from another CDN, `cdn.omniliberal.dev`.
  - The styles can't be split per emote reliably: a rule's subject is often another emote. Rendering them (or not) is the viewer's call.
- **In `dgg-api`:** `assets::css` checks the generated rules against the JSON (rule by rule, not bytewise) and returns the emotes' styles and the flairs' preamble. The live files matched on 2026-11-09 (`examples/fetch_assets.rs`).

## Status
- The chat client (reconnect with backoff, PING/PONG, idle timeout, sending raw frames), behind feature `client`; decoding events and encoding commands, behind `protocol`; emotes and flairs decoded behind `assets`, their stylesheets split into generated and hand-written, and all four fetched from the CDN behind `http` (fresh copies on `REFRESHCHATASSETS`'s cache key).
  - Events seen in a capture (2026-11-09) are typed from it; the rest from chat-gui's handlers (github.com/destinygg/chat-gui), as are the commands.
  - Frames carry no arrival time; the logger stamps them as it receives them. Stamping them in the client, as each is read, would be more accurate. Perhaps later, in `kick-api` too.
  - Logging in: a destiny.gg login key (Profile > Developer > Connections), sent as the cookie `authtoken=<key>`. Tried 2026-11-09 with `examples/send.rs`: `ME` named the user, and a public `MSG` echoed back. Whispers are untried.
