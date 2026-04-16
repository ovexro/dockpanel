// Prometheus exposition format renderer.
//
// Reads the latest snapshot from metrics_history, gpu_metrics_history,
// sites, and alerts and renders text/plain per the Prometheus exposition
// format spec v0.0.4. One scrape = a handful of indexed DB queries; the
// metrics_collector task writes fresh rows every 30s so data is always
// well within Prometheus default scrape intervals (15-60s).

use sqlx::PgPool;
use std::fmt::Write;

/// Render the current metrics snapshot in Prometheus text exposition format.
pub async fn render(pool: &PgPool) -> String {
    let mut out = String::with_capacity(4096);

    render_info(&mut out);
    render_system(&mut out, pool).await;
    render_gpu(&mut out, pool).await;
    render_sites(&mut out, pool).await;
    render_alerts(&mut out, pool).await;

    out
}

fn render_info(out: &mut String) {
    let _ = writeln!(out, "# HELP dockpanel_info DockPanel build information.");
    let _ = writeln!(out, "# TYPE dockpanel_info gauge");
    let _ = write!(out, "dockpanel_info{{version=\"");
    push_escaped(out, env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "\"}} 1");
}

async fn render_system(out: &mut String, pool: &PgPool) {
    let rows: Vec<(uuid::Uuid, String, f32, f32, f32)> = sqlx::query_as(
        "SELECT DISTINCT ON (m.server_id) m.server_id, s.name, \
                m.cpu_pct, m.mem_pct, m.disk_pct \
         FROM metrics_history m \
         JOIN servers s ON s.id = m.server_id \
         WHERE m.server_id IS NOT NULL \
         ORDER BY m.server_id, m.created_at DESC",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if rows.is_empty() {
        return;
    }

    let _ = writeln!(out, "# HELP dockpanel_cpu_percent Current CPU usage (0-100).");
    let _ = writeln!(out, "# TYPE dockpanel_cpu_percent gauge");
    for (sid, name, cpu, _, _) in &rows {
        write_server_sample(out, "dockpanel_cpu_percent", sid, name, *cpu);
    }
    let _ = writeln!(out, "# HELP dockpanel_memory_percent Current memory usage (0-100).");
    let _ = writeln!(out, "# TYPE dockpanel_memory_percent gauge");
    for (sid, name, _, mem, _) in &rows {
        write_server_sample(out, "dockpanel_memory_percent", sid, name, *mem);
    }
    let _ = writeln!(out, "# HELP dockpanel_disk_percent Current disk usage (0-100).");
    let _ = writeln!(out, "# TYPE dockpanel_disk_percent gauge");
    for (sid, name, _, _, disk) in &rows {
        write_server_sample(out, "dockpanel_disk_percent", sid, name, *disk);
    }
}

async fn render_gpu(out: &mut String, pool: &PgPool) {
    let rows: Vec<(uuid::Uuid, String, i16, f32, f32, f32, Option<f32>, Option<f32>)> =
        sqlx::query_as(
            "SELECT DISTINCT ON (g.server_id, g.gpu_index) \
                    g.server_id, s.name, g.gpu_index, \
                    g.utilization_pct, g.memory_used_mb, g.memory_total_mb, \
                    g.temperature_c, g.power_draw_w \
             FROM gpu_metrics_history g \
             JOIN servers s ON s.id = g.server_id \
             WHERE g.server_id IS NOT NULL \
             ORDER BY g.server_id, g.gpu_index, g.created_at DESC",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    if rows.is_empty() {
        return;
    }

    let _ = writeln!(out, "# HELP dockpanel_gpu_utilization_percent GPU compute utilization (0-100).");
    let _ = writeln!(out, "# TYPE dockpanel_gpu_utilization_percent gauge");
    for r in &rows {
        write_gpu_sample(out, "dockpanel_gpu_utilization_percent", &r.0, &r.1, r.2, r.3);
    }
    let _ = writeln!(out, "# HELP dockpanel_gpu_vram_used_mb GPU VRAM used in MB.");
    let _ = writeln!(out, "# TYPE dockpanel_gpu_vram_used_mb gauge");
    for r in &rows {
        write_gpu_sample(out, "dockpanel_gpu_vram_used_mb", &r.0, &r.1, r.2, r.4);
    }
    let _ = writeln!(out, "# HELP dockpanel_gpu_vram_total_mb GPU VRAM capacity in MB.");
    let _ = writeln!(out, "# TYPE dockpanel_gpu_vram_total_mb gauge");
    for r in &rows {
        write_gpu_sample(out, "dockpanel_gpu_vram_total_mb", &r.0, &r.1, r.2, r.5);
    }
    let _ = writeln!(out, "# HELP dockpanel_gpu_temperature_celsius GPU temperature in Celsius.");
    let _ = writeln!(out, "# TYPE dockpanel_gpu_temperature_celsius gauge");
    for r in &rows {
        if let Some(v) = r.6 {
            write_gpu_sample(out, "dockpanel_gpu_temperature_celsius", &r.0, &r.1, r.2, v);
        }
    }
    let _ = writeln!(out, "# HELP dockpanel_gpu_power_draw_watts GPU power draw in Watts.");
    let _ = writeln!(out, "# TYPE dockpanel_gpu_power_draw_watts gauge");
    for r in &rows {
        if let Some(v) = r.7 {
            write_gpu_sample(out, "dockpanel_gpu_power_draw_watts", &r.0, &r.1, r.2, v);
        }
    }
}

async fn render_sites(out: &mut String, pool: &PgPool) {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(status, 'unknown') AS status, COUNT(*) \
         FROM sites GROUP BY status",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if rows.is_empty() {
        return;
    }

    let _ = writeln!(out, "# HELP dockpanel_site_count Sites grouped by status.");
    let _ = writeln!(out, "# TYPE dockpanel_site_count gauge");
    for (status, cnt) in &rows {
        let _ = write!(out, "dockpanel_site_count{{status=\"");
        push_escaped(out, status);
        let _ = writeln!(out, "\"}} {cnt}");
    }
}

async fn render_alerts(out: &mut String, pool: &PgPool) {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT severity, COUNT(*) FROM alerts WHERE status = 'firing' GROUP BY severity",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let _ = writeln!(out, "# HELP dockpanel_alerts_firing Firing alerts grouped by severity.");
    let _ = writeln!(out, "# TYPE dockpanel_alerts_firing gauge");
    if rows.is_empty() {
        // Publish a zero so scrapers can reliably alert on presence.
        let _ = writeln!(out, "dockpanel_alerts_firing{{severity=\"none\"}} 0");
    } else {
        for (severity, cnt) in &rows {
            let _ = write!(out, "dockpanel_alerts_firing{{severity=\"");
            push_escaped(out, severity);
            let _ = writeln!(out, "\"}} {cnt}");
        }
    }
}

// ── Label-line writers ──────────────────────────────────────────────────

fn write_server_sample(out: &mut String, metric: &str, sid: &uuid::Uuid, name: &str, v: f32) {
    let _ = write!(out, "{metric}{{server_id=\"{sid}\",server=\"");
    push_escaped(out, name);
    let _ = writeln!(out, "\"}} {v}");
}

fn write_gpu_sample(
    out: &mut String,
    metric: &str,
    sid: &uuid::Uuid,
    name: &str,
    idx: i16,
    v: f32,
) {
    let _ = write!(out, "{metric}{{server_id=\"{sid}\",server=\"");
    push_escaped(out, name);
    let _ = writeln!(out, "\",gpu_index=\"{idx}\"}} {v}");
}

/// Escape a string for use inside a Prometheus label value.
/// Per the exposition spec: backslash, double-quote, and newline require
/// escape sequences; everything else is pass-through.
fn push_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::push_escaped;

    #[test]
    fn escapes_dangerous_chars() {
        let mut s = String::new();
        push_escaped(&mut s, r#"a"b\c"#);
        assert_eq!(s, r#"a\"b\\c"#);
    }

    #[test]
    fn escapes_newline() {
        let mut s = String::new();
        push_escaped(&mut s, "line1\nline2");
        assert_eq!(s, "line1\\nline2");
    }

    #[test]
    fn passes_through_normal_text() {
        let mut s = String::new();
        push_escaped(&mut s, "my-server-01");
        assert_eq!(s, "my-server-01");
    }
}
