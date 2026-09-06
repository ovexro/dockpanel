-- Withdraw the Stripe subscription-billing surface (routes/billing.rs, deleted
-- alongside this migration). Fully Stripe-wired but never had a UI trigger, and
-- the product's own README/docs assert "Zero subscriptions" / "no premium tier"
-- as a settled identity — see FEATURES.md's Withdrawn Claims table.
DROP INDEX IF EXISTS idx_users_stripe_customer;

ALTER TABLE users
    DROP COLUMN IF EXISTS stripe_customer_id,
    DROP COLUMN IF EXISTS stripe_subscription_id,
    DROP COLUMN IF EXISTS plan,
    DROP COLUMN IF EXISTS plan_status,
    DROP COLUMN IF EXISTS plan_server_limit;

DELETE FROM settings WHERE key IN ('stripe_price_starter', 'stripe_price_pro', 'stripe_price_agency');
