# Archives and stats

Not yet ordered. See the [overall rules](README.md).

Some can likely be derived from chat events the journal already captures (marked *chat*). The rest need other dgg endpoints or sites, which still have to be probed (marked *probe*).
- donations: amounts, users, messages, times (*chat*: `DONATION`);
- gifted subs: gifter, recipients, times (*chat*: `GIFTSUB`/`MASSGIFT`);
- bans, with their reasons (*chat*: `BAN`/`MUTE`);
- pinned messages (*chat*: `PIN`);
- emotes: which are popular, each user's emotes and how often they use them, and rare emotes (*chat*: `MSG` text against the emote list);
  - Halloween and other special-event emotes are counted separately from the regular set. That means keeping a history of the emote list, so it's known which emotes existed when.
  - Greentext (`>` lines) and orangetext are told apart from each other and from emotes. Orangetext is greentext as shown during Halloween events.
    - **Open question:** whether anything in the message marks it as orangetext, or the chat client just restyles greentext during the event. If it's only a client change, tag greentext by whether an event was running at that timestamp, using the emote-list history above or event dates.
- user activity times (*chat*: `MSG`/`JOIN`/`QUIT`);
- user subscription statuses (*chat*: `MSG`/`NAMES` carry `subscription` and `features`);
- TTS messages (*probe*; may arrive as chat events);
- user bios (*probe*);
- dgg profile membership cards (*probe*);
- Vestaboard art (*probe*);
- tweets (*probe*);
- the schedule (*probe*);
- the website's change notes (*probe*).
