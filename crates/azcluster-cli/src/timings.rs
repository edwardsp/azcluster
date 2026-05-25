use crate::arm::client::ArmClient;
use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

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

fn parse_target_id(id: &str) -> (Option<String>, Option<String>, Option<String>) {
    // Two shapes we care about:
    //   /subscriptions/<sub>/resourceGroups/<rg>/providers/<ns>/<type>/<name>[...]
    //   /subscriptions/<sub>/providers/<ns>/<type>/<name>[...]   (sub-scope, no rg)
    let parts: Vec<&str> = id.split('/').filter(|s| !s.is_empty()).collect();
    let mut rg = None;
    if let Some(pos) = parts
        .iter()
        .position(|s| s.eq_ignore_ascii_case("resourceGroups"))
    {
        if let Some(v) = parts.get(pos + 1) {
            rg = Some((*v).to_string());
        }
    }
    let prov = parts
        .iter()
        .position(|s| s.eq_ignore_ascii_case("providers"));
    let (rtype, rname) = match prov {
        Some(p) if parts.len() >= p + 4 => {
            let ns = parts[p + 1];
            let typ = parts[p + 2];
            let name = parts[p + 3];
            (Some(format!("{ns}/{typ}")), Some(name.to_string()))
        }
        _ => (None, None),
    };
    (rg, rtype, rname)
}

fn az_op_to_timing(
    op: &Value,
    deployment: &str,
) -> Option<(OperationTiming, Option<(String, String)>)> {
    let props = op.get("properties")?;
    let duration = props
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(parse_iso8601_duration)
        .unwrap_or(0.0);
    let target = props.get("targetResource");
    let id_str = target
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (id_rg, id_rtype, id_rname) = if id_str.is_empty() {
        (None, None, None)
    } else {
        parse_target_id(id_str)
    };
    let rtype = target
        .and_then(|t| t.get("resourceType"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or(id_rtype)
        .unwrap_or_default();
    let rname = target
        .and_then(|t| t.get("resourceName"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or(id_rname)
        .unwrap_or_default();
    let rg = target
        .and_then(|t| t.get("resourceGroup"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or(id_rg)
        .unwrap_or_default();
    let state = props
        .get("provisioningState")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let nested = if rtype == "Microsoft.Resources/deployments" && !rname.is_empty() {
        // rg may be empty for sub-scope nested deployments; caller handles that.
        Some((rg.clone(), rname.clone()))
    } else {
        None
    };
    Some((
        OperationTiming {
            resource_type: rtype,
            resource_name: rname,
            provisioning_state: state,
            duration_seconds: duration,
            deployment: deployment.to_string(),
        },
        nested,
    ))
}

fn collect_sub_operations(client: &ArmClient, deployment: &str) -> Result<Vec<OperationTiming>> {
    let ops = client.list_subscription_deployment_operations(deployment)?;
    let mut out = Vec::new();
    let mut nested_modules: Vec<(String, String)> = Vec::new();
    for op in ops {
        if let Some((timing, nested)) = az_op_to_timing(&op, deployment) {
            if let Some(n) = nested {
                nested_modules.push(n);
            }
            out.push(timing);
        }
    }
    for (rg, module) in nested_modules {
        let child = if rg.is_empty() {
            collect_sub_operations(client, &module)
        } else {
            collect_group_operations(client, &rg, &module)
        };
        if let Ok(c) = child {
            out.extend(c);
        }
    }
    Ok(out)
}

fn collect_group_operations(
    client: &ArmClient,
    rg: &str,
    module: &str,
) -> Result<Vec<OperationTiming>> {
    let ops = client.list_resource_group_deployment_operations(rg, module)?;
    let mut out = Vec::new();
    let mut nested = Vec::new();
    for op in ops {
        if let Some((timing, child_nested)) = az_op_to_timing(&op, module) {
            if let Some((_, child_name)) = child_nested {
                nested.push(child_name);
            }
            out.push(timing);
        }
    }
    for child_module in nested {
        if let Ok(child) = collect_group_operations(client, rg, &child_module) {
            out.extend(child);
        }
    }
    Ok(out)
}

pub fn capture(
    client: &ArmClient,
    cluster: &str,
    deployment: &str,
    _resource_group: &str,
    shared_storage: &str,
) -> Result<PathBuf> {
    let mut operations = collect_sub_operations(client, deployment).unwrap_or_default();
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
    let total: f64 = operations
        .iter()
        .filter(|o| o.resource_type != "Microsoft.Resources/deployments")
        .map(|o| o.duration_seconds)
        .sum();
    let total = if total == 0.0 { 0.0 } else { total };
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_target_id_rg_scoped() {
        let (rg, t, n) = parse_target_id(
            "/subscriptions/abc/resourceGroups/rg1/providers/Microsoft.Network/virtualNetworks/vnet0",
        );
        assert_eq!(rg.as_deref(), Some("rg1"));
        assert_eq!(t.as_deref(), Some("Microsoft.Network/virtualNetworks"));
        assert_eq!(n.as_deref(), Some("vnet0"));
    }

    #[test]
    fn parse_target_id_sub_scoped_nested_deployment() {
        let (rg, t, n) = parse_target_id(
            "/subscriptions/abc/providers/Microsoft.Resources/deployments/cluster-v211b",
        );
        assert!(rg.is_none());
        assert_eq!(t.as_deref(), Some("Microsoft.Resources/deployments"));
        assert_eq!(n.as_deref(), Some("cluster-v211b"));
    }

    #[test]
    fn az_op_to_timing_falls_back_to_id_when_fields_missing() {
        let op = json!({
            "properties": {
                "duration": "PT8M58.6536497S",
                "provisioningState": "Succeeded",
                "targetResource": {
                    "id": "/subscriptions/abc/providers/Microsoft.Resources/deployments/cluster-v211b"
                }
            }
        });
        let (t, nested) = az_op_to_timing(&op, "outer").unwrap();
        assert_eq!(t.resource_type, "Microsoft.Resources/deployments");
        assert_eq!(t.resource_name, "cluster-v211b");
        assert!((t.duration_seconds - 538.65).abs() < 0.1);
        let (rg, name) = nested.unwrap();
        assert!(rg.is_empty());
        assert_eq!(name, "cluster-v211b");
    }
}
