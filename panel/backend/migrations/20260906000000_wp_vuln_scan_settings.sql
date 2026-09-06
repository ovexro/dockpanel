-- WordPress vulnerability scanning had a manual on-demand endpoint
-- (vuln_scan) but no schedule and no alert — unlike its direct peer, Docker
-- image scanning, which is spawn_supervised'd with a full fire_alert/
-- resolve_alert lifecycle. A site with a critically-vulnerable plugin stayed
-- invisible until an operator manually reopened its toolkit and clicked Scan.
--
-- Settings default to "off" for the same reason image_scan_enabled does: an
-- upgrade must never silently change what an existing install does. Admin
-- opts in via Settings UI.
INSERT INTO settings (key, value) VALUES
    ('wp_vuln_scan_enabled', 'false'),
    ('wp_vuln_scan_interval_hours', '24')
ON CONFLICT (key) DO NOTHING;
