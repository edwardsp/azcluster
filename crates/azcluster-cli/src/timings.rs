use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentTiming {
    pub cluster: String,
    pub deployment: String,
    pub captured_at: String,
    pub shared_storage: String,
    pub total_seconds: f64,
    pub operations: Vec<OperationTiming>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationTiming {
    pub resource_type: String,
    pub resource_name: String,
    pub provisioning_state: String,
    pub duration_seconds: f64,
    pub deployment: String,
}

fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "azcluster").ok_or_else(|| anyhow!("cannot resolve XDG config dir"))
}

pub fn deployments_dir(cluster: &str) -> Result<PathBuf> {
    Ok(dirs()?.config_dir().join("deployments").join(cluster))
}

fn parse_iso8601_duration(s: &str) -> Option<f64> {
    let s = s.strip_prefix("PT")?;
    let mut total: f64 = 0.0;
    let mut buf = String::new();
    for ch in s.chars() {
        match ch {
            '0'..='9' | '.' => buf.push(ch),
            'H' => {
                total += buf.parse::<f64>().ok()? * 3600.0;
                buf.clear();
            }
            'M' => {
                total += buf.parse::<f64>().ok()? * 60.0;
                buf.clear();
            }
            'S' => {
                total += buf.parse::<f64>().ok()?;
                buf.clear();
            }
            _ => return None,
        }
    }
    Some(total)
}

fn az_json(args: &[&str]) -> Result<Value> {
    let output = Command::new("az")
        .args(args)
        .output()
        .with_context(|| format!("spawn az {}", args.join(" ")))?;
    if !output.status.success() {
        return Err(anyhow!(
            "az {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout).context("parse az json output")
}

fn collect_sub_operations(deployment: &str) -> Result<Vec<OperationTiming>> {
    let ops = az_json(&[
        "deployment",
        "operation",
        "sub",
        "list",
        "--name",
        deployment,
        "-o",
        "json",
    ])?;
    let arr = ops.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    let mut nested_modules: Vec<(String, String)> = Vec::new();
    for op in arr {
        let props = match op.get("properties") {
            Some(p) => p,
            None => continue,
        };
        let duration = props
            .get("duration")
            .and_then(|v| v.as_str())
            .and_then(parse_iso8601_duration)
            .unwrap_or(0.0);
        let target = props.get("targetResource");
        let rtype = target
            .and_then(|t| t.get("resourceType"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rname = target
            .and_then(|t| t.get("resourceName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rg = target
            .and_then(|t| t.get("resourceGroup"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let state = props
            .get("provisioningState")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if rtype == "Microsoft.Resources/deployments" && !rg.is_empty() {
            nested_modules.push((rg.clone(), rname.clone()));
        }
        out.push(OperationTiming {
            resource_type: rtype,
            resource_name: rname,
            provisioning_state: state,
            duration_seconds: duration,
            deployment: deployment.to_string(),
        });
    }
    for (rg, module) in nested_modules {
        if let Ok(child) = collect_group_operations(&rg, &module) {
            out.extend(child);
        }
    }
    Ok(out)
}

fn collect_group_operations(rg: &str, module: &str) -> Result<Vec<OperationTiming>> {
    let ops = az_json(&[
        "deployment",
        "operation",
        "group",
        "list",
        "--resource-group",
        rg,
        "--name",
        module,
        "-o",
        "json",
    ])?;
    let arr = ops.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    let mut nested = Vec::new();
    for op in arr {
        let props = match op.get("properties") {
            Some(p) => p,
            None => continue,
        };
        let duration = props
            .get("duration")
            .and_then(|v| v.as_str())
            .and_then(parse_iso8601_duration)
            .unwrap_or(0.0);
        let target = props.get("targetResource");
        let rtype = target
            .and_then(|t| t.get("resourceType"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rname = target
            .and_then(|t| t.get("resourceName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let state = props
            .get("provisioningState")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if rtype == "Microsoft.Resources/deployments" {
            nested.push(rname.clone());
        }
        out.push(OperationTiming {
            resource_type: rtype,
            resource_name: rname,
            provisioning_state: state,
            duration_seconds: duration,
            deployment: module.to_string(),
        });
    }
    for child_module in nested {
        if let Ok(child) = collect_group_operations(rg, &child_module) {
            out.extend(child);
        }
    }
    Ok(out)
}

pub fn capture(
    cluster: &str,
    deployment: &str,
    _resource_group: &str,
    shared_storage: &str,
) -> Result<PathBuf> {
    let mut operations = collect_sub_operations(deployment).unwrap_or_default();
    operations.retain(|o| !o.resource_type.is_empty());
    operations.sort_by(|a, b| {
        (&a.resource_type, &a.resource_name, &a.deployment)
            .cmp(&(&b.resource_type, &b.resource_name, &b.deployment))
            .then(
                b.duration_seconds
                    .partial_cmp(&a.duration_seconds)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    operations.dedup_by(|a, b| {
        a.resource_type == b.resource_type
            && a.resource_name == b.resource_name
            && a.deployment == b.deployment
    });
    let total = operations
        .iter()
        .filter(|o| o.resource_type != "Microsoft.Resources/deployments")
        .map(|o| o.duration_seconds)
        .sum();
    let stamp = current_utc_stamp();
    let timing = DeploymentTiming {
        cluster: cluster.to_string(),
        deployment: deployment.to_string(),
        captured_at: stamp.clone(),
        shared_storage: shared_storage.to_string(),
        total_seconds: total,
        operations,
    };
    let dir = deployments_dir(cluster)?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{stamp}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&timing)?)?;
    append_trend(&dir, &timing)?;
    eprintln!(
        "==> deployment timing captured ({} resources, total {:.0}s) -> {}",
        timing.operations.len(),
        timing.total_seconds,
        path.display()
    );
    Ok(path)
}

fn append_trend(dir: &std::path::Path, t: &DeploymentTiming) -> Result<()> {
    let path = dir.join("trend.tsv");
    let need_header = !path.exists();
    let mut line = String::new();
    if need_header {
        line.push_str("captured_at\tdeployment\tshared_storage\ttotal_seconds\tresource_count\n");
    }
    line.push_str(&format!(
        "{}\t{}\t{}\t{:.1}\t{}\n",
        t.captured_at,
        t.deployment,
        t.shared_storage,
        t.total_seconds,
        t.operations.len()
    ));
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

fn current_utc_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, mon, day, h, m, s) = epoch_to_ymdhms(now);
    format!("{year:04}-{mon:02}-{day:02}T{h:02}{m:02}{s:02}Z")
}

fn epoch_to_ymdhms(mut t: u64) -> (i32, u32, u32, u32, u32, u32) {
    let s = (t % 60) as u32;
    t /= 60;
    let m = (t % 60) as u32;
    t /= 60;
    let h = (t % 24) as u32;
    t /= 24;
    let mut days = t as i64;
    let mut year: i32 = 1970;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let dy = if leap { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let dim = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mon: u32 = 1;
    for &d in &dim {
        if days < d {
            break;
        }
        days -= d;
        mon += 1;
    }
    let day = (days as u32) + 1;
    (year, mon, day, h, m, s)
}

pub fn list_for_cluster(cluster: &str, last: usize) -> Result<Vec<DeploymentTiming>> {
    let dir = deployments_dir(cluster)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    entries.reverse();
    let take = if last == 0 { entries.len() } else { last };
    let mut out = Vec::new();
    for e in entries.into_iter().take(take) {
        let body = std::fs::read_to_string(e.path())?;
        let t: DeploymentTiming = serde_json::from_str(&body)?;
        out.push(t);
    }
    Ok(out)
}

pub fn print_table(t: &DeploymentTiming) {
    println!(
        "deployment: {}  captured: {}  shared_storage: {}  total: {:.1}s",
        t.deployment, t.captured_at, t.shared_storage, t.total_seconds
    );
    let mut ops = t.operations.clone();
    ops.sort_by(|a, b| {
        b.duration_seconds
            .partial_cmp(&a.duration_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    println!(
        "{:>8}  {:<48}  {:<32}  state",
        "secs", "resource_type", "name"
    );
    for op in ops {
        println!(
            "{:>8.1}  {:<48}  {:<32}  {}",
            op.duration_seconds,
            truncate(&op.resource_type, 48),
            truncate(&op.resource_name, 32),
            op.provisioning_state
        );
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
