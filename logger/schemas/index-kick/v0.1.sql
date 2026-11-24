-- Kick's index schema version 0.1, the first and current one
-- (logger::kick::index::SCHEMA_VERSION): every Kick channel's journal, in
-- index-kick.sqlite, apart from dgg's index.
--
-- Rows are keyed by Kick's own ids where it gives them, so captures from
-- several hosts, and the messages a gap was filled with, merge simply.

CREATE TABLE files (
  path TEXT PRIMARY KEY,   -- relative to dgg's journal root, '/'-separated:
                           -- kick/<slug>/...
  offset INTEGER NOT NULL, -- just past the last indexed record
  digest TEXT              -- an archive's SHA-256 when read; NULL if live
);
CREATE TABLE messages (
  id TEXT PRIMARY KEY,       -- Kick's uuid
  channel TEXT NOT NULL,     -- its slug
  sent INTEGER,              -- when Kick says, unix ms (whole seconds)
  received INTEGER NOT NULL, -- the earliest local receive time, unix ms
  user_id INTEGER NOT NULL,
  username TEXT NOT NULL,    -- as it was then
  content TEXT NOT NULL,     -- emotes inline as [emote:<id>:<name>]
  type TEXT,                 -- 'message' or 'reply'
  source TEXT NOT NULL       -- 'live', or 'history' if a gap was filled
                             -- with it and it was never received live
);
CREATE INDEX messages_user ON messages (channel, user_id, sent);
CREATE INDEX messages_sent ON messages (channel, sent);
-- A message deleted, by whom unknown.
CREATE TABLE deletions (
  message_id TEXT PRIMARY KEY,
  id TEXT,                   -- the deletion's own uuid, if given
  channel TEXT NOT NULL,
  received INTEGER NOT NULL, -- the earliest local receive time, unix ms
  ai INTEGER NOT NULL,       -- 1 if Kick's AI moderation deleted it
  rules TEXT                 -- JSON array of the rules the AI found broken
);
-- Bans, timeouts and unbans.
CREATE TABLE bans (
  id TEXT UNIQUE,            -- Kick's uuid, if given
  channel TEXT NOT NULL,
  received INTEGER NOT NULL, -- the earliest local receive time, unix ms
  kind TEXT NOT NULL,        -- 'ban', 'timeout' or 'unban'
  user_id INTEGER NOT NULL,
  username TEXT NOT NULL,
  by TEXT,                   -- the moderator's username, if given
  minutes INTEGER,           -- a timeout's
  until INTEGER,             -- when a timeout ends, unix ms
  permanent INTEGER          -- an unban's: 1 if it lifted a ban
);
CREATE INDEX bans_user ON bans (channel, user_id, received);
CREATE TABLE unparsed (
  path TEXT NOT NULL,
  offset INTEGER NOT NULL,
  item INTEGER NOT NULL, -- index within a frame or history page, else 0
  reason TEXT NOT NULL,
  PRIMARY KEY (path, offset, item)
);
