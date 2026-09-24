-- Migration 2 (M1E): pairing and devices, with their invariants in the schema.
--
-- pairing_state is rebuilt. The armed secret is kept only as XChaCha20-Poly1305
-- ciphertext under a key derived from /etc/atrium/secrets.key (ADR-019); the
-- migration-1 `secret_digest` column goes, because a digest cannot verify a
-- proof and has no remaining purpose. The CHECK makes the two legal shapes the
-- only shapes: armed, with every field of the sealed secret present and the
-- only implemented profile; or not armed, with none of them. A locked state is
-- a disarmed one: the lock destroys the secret.
--
-- devices is rebuilt with CHECKs for what the code already guarantees, so a
-- bug cannot store a malformed id, an unbounded name, a role other than
-- owner, or a verifier of the wrong size.
--
-- Rows that violate a new constraint make the migration fail and roll back
-- (recovery mode, backup kept); nothing is silently dropped. M1 before this
-- migration never armed pairing or created a device, so in practice both
-- tables carry nothing but the pairing_state defaults.

CREATE TABLE pairing_state_m2 (
  id                INTEGER PRIMARY KEY CHECK (id = 1),
  claimed           INTEGER NOT NULL DEFAULT 0 CHECK (claimed IN (0, 1)),
  armed             INTEGER NOT NULL DEFAULT 0 CHECK (armed IN (0, 1)),
  arming_id         BLOB,
  profile           TEXT,
  secret_nonce      BLOB,
  secret_ciphertext BLOB,
  armed_at          TEXT,
  expires_at        TEXT,
  failures          INTEGER NOT NULL DEFAULT 0 CHECK (failures BETWEEN 0 AND 5),
  locked            INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
  CHECK (
    (armed = 1
      AND typeof(arming_id) = 'blob' AND length(arming_id) = 16
      AND profile = 'atrium-pair-binding/native-tls-exporter-v1'
      AND typeof(secret_nonce) = 'blob' AND length(secret_nonce) = 24
      AND typeof(secret_ciphertext) = 'blob' AND length(secret_ciphertext) = 32
      AND armed_at IS NOT NULL AND expires_at IS NOT NULL
      AND locked = 0 AND failures < 5)
    OR
    (armed = 0
      AND arming_id IS NULL AND profile IS NULL
      AND secret_nonce IS NULL AND secret_ciphertext IS NULL
      AND armed_at IS NULL AND expires_at IS NULL)
  )
);

-- Whatever migration 1 recorded as armed carried only a digest, which cannot
-- verify a proof: it comes across disarmed. The claim and a lock survive.
INSERT INTO pairing_state_m2 (id, claimed, armed, failures, locked)
  SELECT id, claimed, 0, 0, locked FROM pairing_state;
INSERT OR IGNORE INTO pairing_state_m2 (id) VALUES (1);
DROP TABLE pairing_state;
ALTER TABLE pairing_state_m2 RENAME TO pairing_state;

CREATE TABLE devices_m2 (
  device_id     TEXT PRIMARY KEY
                CHECK (length(device_id) = 32 AND device_id NOT GLOB '*[^0-9a-f]*'),
  name          TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64 AND length(CAST(name AS BLOB)) <= 128),
  platform      TEXT NOT NULL CHECK (platform IN ('windows', 'macos', 'linux', 'web', 'other')),
  role          TEXT NOT NULL CHECK (role = 'owner'),
  token_digest  BLOB NOT NULL UNIQUE CHECK (typeof(token_digest) = 'blob' AND length(token_digest) = 32),
  created_at    TEXT NOT NULL,
  last_seen_at  TEXT
);
INSERT INTO devices_m2 (device_id, name, platform, role, token_digest, created_at, last_seen_at)
  SELECT device_id, name, platform, role, token_digest, created_at, last_seen_at FROM devices;
DROP TABLE devices;
ALTER TABLE devices_m2 RENAME TO devices;
