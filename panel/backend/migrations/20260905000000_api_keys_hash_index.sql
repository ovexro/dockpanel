-- authenticate_api_key() (auth.rs) looks up every dp_-prefixed bearer token by
-- `WHERE key_hash = $1` on every request that uses one — the migration that
-- created this table indexed key_prefix (never queried by) and user_id, but
-- not the column the actual hot-path lookup filters on, forcing a sequential
-- scan of the whole table on every API-key-authenticated request. UNIQUE
-- because two different keys colliding on the same SHA-256 hash would let one
-- authenticate as the other's owner — worth a DB-level guarantee, not just
-- the astronomical improbability of a real collision.
CREATE UNIQUE INDEX idx_api_keys_hash ON api_keys(key_hash);
