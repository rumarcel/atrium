-- Migration 3: no failed-proof lock (ADR-020).
--
-- A failed proof consumes its attempt and nothing else, so `locked` has no
-- meaning and `failures` (capped at 5 by migration 2) becomes
-- `failed_attempts`: an uncapped, per-arming count for the audit row that
-- ends the arming, never read by any decision and never served by the API.
-- It is zero whenever nothing is armed.
--
-- An armed row keeps its sealed secret, whose associated data never included
-- the count (ADR-019), and its count. A locked row was already disarmed and
-- comes across as simply disarmed.

CREATE TABLE pairing_state_m3 (
  id                INTEGER PRIMARY KEY CHECK (id = 1),
  claimed           INTEGER NOT NULL DEFAULT 0 CHECK (claimed IN (0, 1)),
  armed             INTEGER NOT NULL DEFAULT 0 CHECK (armed IN (0, 1)),
  arming_id         BLOB,
  profile           TEXT,
  secret_nonce      BLOB,
  secret_ciphertext BLOB,
  armed_at          TEXT,
  expires_at        TEXT,
  failed_attempts   INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
  CHECK (
    (armed = 1
      AND typeof(arming_id) = 'blob' AND length(arming_id) = 16
      AND profile = 'atrium-pair-binding/native-tls-exporter-v1'
      AND typeof(secret_nonce) = 'blob' AND length(secret_nonce) = 24
      AND typeof(secret_ciphertext) = 'blob' AND length(secret_ciphertext) = 32
      AND armed_at IS NOT NULL AND expires_at IS NOT NULL)
    OR
    (armed = 0
      AND arming_id IS NULL AND profile IS NULL
      AND secret_nonce IS NULL AND secret_ciphertext IS NULL
      AND armed_at IS NULL AND expires_at IS NULL
      AND failed_attempts = 0)
  )
);

INSERT INTO pairing_state_m3 (id, claimed, armed, arming_id, profile, secret_nonce,
                              secret_ciphertext, armed_at, expires_at, failed_attempts)
  SELECT id, claimed, armed, arming_id, profile, secret_nonce, secret_ciphertext,
         armed_at, expires_at, CASE WHEN armed = 1 THEN failures ELSE 0 END
  FROM pairing_state;
INSERT OR IGNORE INTO pairing_state_m3 (id) VALUES (1);
DROP TABLE pairing_state;
ALTER TABLE pairing_state_m3 RENAME TO pairing_state;
