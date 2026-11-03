-- v2 adds environment/global/collection configuration to the session JSON,
-- and API Key, inherited auth, forms and per-request options to request JSON.
-- Version gating prevents older clients from silently dropping these fields.
BEGIN IMMEDIATE;
PRAGMA user_version = 2;
COMMIT;
