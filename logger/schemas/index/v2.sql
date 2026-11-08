-- Index schema version 2, the current one (logger::index::SCHEMA_VERSION).
--
-- Changes from v1: connections gains last_seen and history_from.

CREATE TABLE files (
  path TEXT PRIMARY KEY,  -- relative to the journal root, '/'-separated
  offset INTEGER NOT NULL -- just past the last indexed line
);
CREATE TABLE connections (
  instance TEXT NOT NULL,
  connection INTEGER NOT NULL,
  url TEXT NOT NULL,
  opened INTEGER NOT NULL,
  closed INTEGER,
  close_reason TEXT,
  last_seen INTEGER NOT NULL,  -- receive time of its latest line
  history_from INTEGER,        -- oldest MSG timestamp in its HISTORY
  PRIMARY KEY (instance, connection)
);
CREATE TABLE messages (
  id INTEGER PRIMARY KEY,
  timestamp INTEGER NOT NULL, -- dgg's, unix ms
  nick TEXT NOT NULL,
  data TEXT NOT NULL,
  features TEXT NOT NULL,     -- JSON array, as sent
  received INTEGER NOT NULL,  -- local receive time, unix ms
  source TEXT NOT NULL,       -- 'live' or 'history'
  UNIQUE (timestamp, nick, data)
);
CREATE INDEX messages_nick ON messages (nick, timestamp);
CREATE VIRTUAL TABLE messages_fts
  USING fts5(data, content = 'messages', content_rowid = 'id');
CREATE TABLE unparsed (
  path TEXT NOT NULL,
  offset INTEGER NOT NULL,
  item INTEGER NOT NULL, -- index within a HISTORY frame, else 0
  reason TEXT NOT NULL,
  PRIMARY KEY (path, offset, item)
);
