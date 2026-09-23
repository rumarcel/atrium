-- Migration 1: the M1 schema, exactly as docs/M1-IMPLEMENTATION-PLAN.md
-- section 4.3 states it. The PRAGMA lines from that section are applied to
-- every connection by the code that opens the database, not here: SQLite
-- cannot change the journal mode inside the transaction a migration runs in.

CREATE TABLE devices (
  device_id     TEXT PRIMARY KEY,          -- 16 random bytes, lowercase hex
  name          TEXT NOT NULL,
  platform      TEXT NOT NULL,             -- closed enum: windows|macos|linux|web|other
  role          TEXT NOT NULL,             -- M1: always 'owner'
  token_digest  BLOB NOT NULL UNIQUE,      -- SHA-256(token), 32 bytes, the whole verifier
  created_at    TEXT NOT NULL,
  last_seen_at  TEXT
);

CREATE TABLE pairing_state (
  id            INTEGER PRIMARY KEY CHECK (id = 1),
  claimed       INTEGER NOT NULL DEFAULT 0,
  armed         INTEGER NOT NULL DEFAULT 0,
  secret_digest BLOB,                      -- SHA-256 of the decoded 16 bytes
  armed_at      TEXT,
  expires_at    TEXT,
  failures      INTEGER NOT NULL DEFAULT 0,
  locked        INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE audit (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  ts            TEXT NOT NULL,
  request_id    TEXT,
  actor_device  TEXT,                      -- NULL for console/system actions
  actor_role    TEXT,
  action        TEXT NOT NULL,             -- closed enum
  target        TEXT,
  outcome       TEXT NOT NULL,             -- ok | failed | refused
  error_code    TEXT,
  detail        TEXT                       -- JSON, redacted by construction
);

CREATE TABLE settings (
  key           TEXT PRIMARY KEY,
  value         TEXT NOT NULL
);
