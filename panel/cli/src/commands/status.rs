use crate::client;

pub fn usage_color(pct: f64) -> &'static str {
    if pct > 90.0 {
        "\x1b[31m" // red
    } else if pct > 70.0 {
        "\x1b[33m" // yellow
    } else {
        "\x1b[32m" // green
    }
}

pub async fn cmd_status(token: &str, output: &str) -> Result<(), String> {
    let info = client::agent_get("/system/info", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
        return Ok(());
    }

    let hostname = info["hostname"].as_str().unwrap_or("unknown");
    let os = info["os"].as_str().unwrap_or("unknown");
    let kernel = info["kernel"].as_str().unwrap_or("unknown");
    let cpu_model = info["cpu_model"].as_str().unwrap_or("unknown");
    let cpu_count = info["cpu_count"].as_u64().unwrap_or(0);
    let cpu_usage = info["cpu_usage"].as_f64().unwrap_or(0.0);
    let mem_total = info["mem_total_mb"].as_u64().unwrap_or(0);
    let mem_used = info["mem_used_mb"].as_u64().unwrap_or(0);
    let mem_pct = info["mem_usage_pct"].as_f64().unwrap_or(0.0);
    let disk_total = info["disk_total_gb"].as_f64().unwrap_or(0.0);
    let disk_used = info["disk_used_gb"].as_f64().unwrap_or(0.0);
    let disk_pct = info["disk_usage_pct"].as_f64().unwrap_or(0.0);
    let uptime = info["uptime_secs"].as_u64().unwrap_or(0);
    let load1 = info["load_avg_1"].as_f64().unwrap_or(0.0);
    let load5 = info["load_avg_5"].as_f64().unwrap_or(0.0);
    let load15 = info["load_avg_15"].as_f64().unwrap_or(0.0);
    let procs = info["process_count"].as_u64().unwrap_or(0);

    let days = uptime / 86400;
    let hours = (uptime % 86400) / 3600;
    let mins = (uptime % 3600) / 60;

    println!("\x1b[1mServer Status\x1b[0m");
    println!("  Hostname:    {hostname}");
    println!("  OS:          {os}");
    println!("  Kernel:      {kernel}");
    println!("  Uptime:      {days}d {hours}h {mins}m");
    println!();
    println!("\x1b[1mCPU\x1b[0m");
    println!("  Model:       {cpu_model}");
    println!("  Cores:       {cpu_count}");
    println!(
        "  Usage:       {}{:.1}%\x1b[0m",
        usage_color(cpu_usage),
        cpu_usage
    );
    println!("  Load:        {load1:.2} / {load5:.2} / {load15:.2}");
    println!();
    println!("\x1b[1mMemory\x1b[0m");
    println!(
        "  Used:        {}{mem_used} MB\x1b[0m / {mem_total} MB ({mem_pct:.1}%)",
        usage_color(mem_pct)
    );
    println!();
    println!("\x1b[1mDisk\x1b[0m");
    println!(
        "  Used:        {}{disk_used:.1} GB\x1b[0m / {disk_total:.1} GB ({disk_pct:.1}%)",
        usage_color(disk_pct)
    );
    println!();
    println!("  Processes:   {procs}");

    Ok(())
}

pub async fn cmd_top(token: &str, output: &str) -> Result<(), String> {
    let procs = client::agent_get("/system/processes", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&procs).unwrap_or_default());
        return Ok(());
    }

    let procs = procs
        .as_array()
        .ok_or("Expected array from /system/processes")?;

    println!(
        "\x1b[1m{:<8} {:<30} {:<10} {:<10}\x1b[0m",
        "PID", "NAME", "CPU %", "MEM MB"
    );

    for p in procs {
        let pid = p["pid"].as_u64().unwrap_or(0);
        let name = p["name"].as_str().unwrap_or("-");
        let cpu = p["cpu_pct"].as_f64().unwrap_or(0.0);
        let mem = p["mem_mb"].as_f64().unwrap_or(0.0);

        let cpu_color = usage_color(cpu);
        println!(
            "{:<8} {:<30} {cpu_color}{:<10.1}\x1b[0m {:<10.1}",
            pid, name, cpu, mem
        );
    }

    Ok(())
}

pub async fn cmd_services(token: &str, output: &str, filter: Option<&str>) -> Result<(), String> {
    let svcs = client::agent_get("/services/health", token).await?;
    let svcs = svcs
        .as_array()
        .ok_or("Expected array from /services/health")?;

    // Apply filter
    let filtered: Vec<_> = if let Some(f) = filter {
        let f_lower = f.to_lowercase();
        svcs.iter()
            .filter(|svc| {
                svc["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&f_lower)
            })
            .collect()
    } else {
        svcs.iter().collect()
    };

    if output == "json" {
        let json_arr: Vec<_> = filtered.into_iter().cloned().collect();
        println!("{}", serde_json::to_string_pretty(&json_arr).unwrap_or_default());
        return Ok(());
    }

    println!("\x1b[1m{:<25} {:<15}\x1b[0m", "SERVICE", "STATUS");

    for svc in &filtered {
        let name = svc["name"].as_str().unwrap_or("-");
        let status = svc["status"].as_str().unwrap_or("-");

        let color = match status {
            "running" => "\x1b[32m",
            "stopped" | "failed" => "\x1b[31m",
            "disabled" | "not_installed" => "\x1b[90m",
            _ => "\x1b[33m",
        };

        println!("{:<25} {color}{:<15}\x1b[0m", name, status);
    }

    Ok(())
}

pub async fn cmd_diagnose(token: &str, output: &str) -> Result<(), String> {
    let report = client::agent_get("/diagnostics", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        return Ok(());
    }

    println!("Running diagnostics...\n");

    let summary = &report["summary"];
    let critical = summary["critical"].as_u64().unwrap_or(0);
    let warning = summary["warning"].as_u64().unwrap_or(0);
    let info = summary["info"].as_u64().unwrap_or(0);
    let total = summary["total"].as_u64().unwrap_or(0);

    if total == 0 {
        println!("\x1b[32m✓ All checks passed — no issues detected.\x1b[0m");
        return Ok(());
    }

    // Summary line
    print!("Found ");
    if critical > 0 {
        print!("\x1b[31m{critical} critical\x1b[0m");
    }
    if warning > 0 {
        if critical > 0 { print!(", "); }
        print!("\x1b[33m{warning} warning\x1b[0m");
    }
    if info > 0 {
        if critical > 0 || warning > 0 { print!(", "); }
        print!("\x1b[34m{info} info\x1b[0m");
    }
    println!(" ({total} total)\n");

    // Group findings by category
    let findings = report["findings"].as_array().unwrap_or(&Vec::new()).clone();
    let categories = ["nginx", "resources", "services", "ssl", "logs", "security"];
    let category_labels = [
        ("nginx", "NGINX"),
        ("resources", "RESOURCES"),
        ("services", "SERVICES"),
        ("ssl", "SSL CERTIFICATES"),
        ("logs", "LOG ANALYSIS"),
        ("security", "SECURITY"),
    ];

    for (cat, label) in &category_labels {
        let cat_findings: Vec<_> = findings
            .iter()
            .filter(|f| f["category"].as_str() == Some(cat))
            .collect();

        if cat_findings.is_empty() {
            continue;
        }

        println!("\x1b[1m{label}\x1b[0m");
        for f in &cat_findings {
            let severity = f["severity"].as_str().unwrap_or("info");
            let title = f["title"].as_str().unwrap_or("");
            let desc = f["description"].as_str().unwrap_or("");
            let fix = f["fix_available"].as_bool() == Some(true);

            let icon = match severity {
                "critical" => "\x1b[31m✗\x1b[0m",
                "warning" => "\x1b[33m!\x1b[0m",
                _ => "\x1b[34mi\x1b[0m",
            };

            println!("  {icon} {title}");
            println!("    {desc}");
            if fix {
                if let Some(fix_id) = f["fix_id"].as_str() {
                    println!("    \x1b[2m(fixable: {fix_id})\x1b[0m");
                }
            }
        }
        println!();
    }

    // Drop unused binding
    let _ = categories;

    Ok(())
}
