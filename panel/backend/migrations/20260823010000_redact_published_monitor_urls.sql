-- Remove monitor URLs that were already stored, and published, before v2.149.0.
--
-- A failed HTTP check stored `reqwest::Error`'s Display verbatim, and that
-- Display ends with " for url ({url})". So the monitor's address, path and
-- query string were written into four columns — and `/api/status-page/public`
-- serves `incidents.cause` and the "Auto-detected: …" entry in
-- `incident_updates.message` to anyone on the internet with no login, while
-- `docs/guides/status-page.md` stated that URLs are not published. A monitor
-- URL routinely carries a token in its query string.
--
-- The writer is fixed in the same release; this closes the rows already
-- written. Two windows kept them live: `auto_incidents` publishes the last 7
-- days, and a resolved managed incident stays up for `history_days` (90 by
-- default). At rest they never expired at all.
--
-- Anchored on the exact phrase reqwest emits, so it cannot touch an operator's
-- own prose — a hand-written incident description that happens to mention a URL
-- is left alone. The pre-v2.149.0 writer never walked the error's source chain,
-- so " for url (…)" is the only shape a historical row can carry.
--
-- ⚠ What this cannot do: un-publish anything already fetched, and reach a
-- backup taken before it ran. The release note tells operators to rotate any
-- credential that was in a monitor URL rather than treating this as sufficient.

UPDATE incidents
   SET cause = regexp_replace(cause, ' for url \([^)]*\)', '', 'g')
 WHERE cause LIKE '% for url (%';

UPDATE managed_incidents
   SET description = regexp_replace(description, ' for url \([^)]*\)', '', 'g')
 WHERE description LIKE '% for url (%';

UPDATE incident_updates
   SET message = regexp_replace(message, ' for url \([^)]*\)', '', 'g')
 WHERE message LIKE '% for url (%';

-- Authenticated-only and purged after 24 hours, so this row set is the least
-- urgent of the four — but it is the same secret sitting in the same database,
-- and the LIKE filter keeps the scan proportional to the damage.
UPDATE monitor_checks
   SET error = regexp_replace(error, ' for url \([^)]*\)', '', 'g')
 WHERE error LIKE '% for url (%';
