-- Two writers spelled the warning level "warn" while every reader spells it
-- "warning".
--
-- `routes/system_logs.rs` filters on ["error","warning","info"] and its `count`
-- endpoint matches the same three, discarding anything else with a `_ => {}`
-- arm. So a "warn" row was invisible three ways at once: absent from the
-- Warnings tile, unreachable through the level filter, and rendered with the
-- neutral grey badge the frontend uses for levels it does not recognise.
--
-- The writers are fixed in the same commit (`backup_policy_executor.rs` and
-- `backup_scheduler.rs` — the latter already spelled it "warning" at two of its
-- three call sites). This normalises the rows they already wrote so an operator
-- upgrading does not keep a tail of warnings that cannot be counted or found.
--
-- Bounded by construction: it touches only rows whose level is exactly the short
-- spelling, a value no reader in the codebase accepts.

UPDATE system_logs SET level = 'warning' WHERE level = 'warn';
