-- Index schema version 0.5, the current one (logger::index::SCHEMA_VERSION).
--
-- Changes from v0.4: none to the tables. Archives' offsets are into
-- `journal dgg.0.2` files, records ending at RS LF; `dgg-logger migrate`
-- upgrades an index at 0.4 as it converts the archives.

CREATE TABLE files (
  path TEXT PRIMARY KEY,   -- relative to the journal root, '/'-separated
  offset INTEGER NOT NULL, -- just past the last indexed record
  digest TEXT              -- an archive's SHA-256 when read; NULL if live
);
CREATE TABLE connections (
  instance TEXT NOT NULL,
  connection INTEGER NOT NULL,
  url TEXT NOT NULL,
  opened INTEGER NOT NULL,
  closed INTEGER,
  close_reason TEXT,
  last_seen INTEGER NOT NULL,  -- receive time of its latest record
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
  source TEXT NOT NULL,       -- 'live', 'history' or 'backfill:<source>'
  UNIQUE (timestamp, nick, data)
);
CREATE INDEX messages_nick ON messages (nick, timestamp);
-- Backfilled nicks are lowercased; match and re-case them through this.
CREATE INDEX messages_nick_lower ON messages (lower(nick));
CREATE VIRTUAL TABLE messages_fts
  USING fts5(data, content = 'messages', content_rowid = 'id');
CREATE TABLE unparsed (
  path TEXT NOT NULL,
  offset INTEGER NOT NULL,
  item INTEGER NOT NULL, -- index within a HISTORY frame, else 0
  reason TEXT NOT NULL,
  PRIMARY KEY (path, offset, item)
);
-- Spans a source returned in full, unix ms.
CREATE TABLE fetch_ranges (
  source TEXT NOT NULL,
  start_time INTEGER NOT NULL,
  end_time INTEGER NOT NULL,
  PRIMARY KEY (source, start_time, end_time)
);
