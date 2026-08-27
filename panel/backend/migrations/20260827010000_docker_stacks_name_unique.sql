-- Give `docker_stacks.name` the uniqueness `20260319000000_multi_server.sql`
-- promised and never delivered.
--
-- That migration DROPPED the table's original global unique constraint under a
-- comment reading "docker_stacks name — unique per server+user" — the same
-- sentence, same shape, as the two lines above it for `dns_zones.domain` and
-- `mail_domains.domain`. Both of those got their scoped replacement three lines
-- later in the same file. `docker_stacks` did not: the DROP landed and nothing
-- ever followed it, so from that migration forward two stacks belonging to the
-- same user on the same server could carry the identical name with nothing to
-- stop it.
--
-- Low blast radius, not zero: `name` is never used as a lookup key (every
-- reader, writer and deleter of a stack goes through its `id`; Docker-level
-- isolation is scoped by `id` too, in `stack_scope()` in the agent's
-- `services/compose.rs`), so a duplicate cannot misdirect a mutation the way a
-- duplicate domain or port already did (20260818000000). What it does do is
-- make Apps.tsx's stack list show two identical rows a click apart, with
-- nothing but the URL to tell them apart — a defect in its own right, and the
-- reason this file exists.
--
-- ── Dedup before enforcing, per this project's own rule ─────────────────────
-- (20260818000000: "a migration that aborts is worse than a column that stays
-- nullable" — the same principle applies to a CREATE UNIQUE INDEX that would
-- abort the whole deploy on any installed box that already has a collision.
-- `name` has had no scoped uniqueness of any kind since the DROP above, so an
-- existing duplicate is a real possibility on a box that has been upgrading
-- since before this fix, not a hypothetical.) Every row but the oldest in a
-- colliding group is renamed with its position appended, deterministically and
-- without operator input — a rename an operator can freely undo from the UI,
-- which a failed migration would not have let them reach in the first place.
WITH ranked AS (
    SELECT id,
           ROW_NUMBER() OVER (
               PARTITION BY user_id, server_id, lower(name)
               ORDER BY created_at, id
           ) AS rn
    FROM docker_stacks
)
UPDATE docker_stacks s
   SET name = s.name || ' (' || ranked.rn || ')'
  FROM ranked
 WHERE ranked.id = s.id
   AND ranked.rn > 1;

CREATE UNIQUE INDEX IF NOT EXISTS idx_docker_stacks_user_server_name
    ON docker_stacks(user_id, server_id, lower(name));
