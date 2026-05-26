use crate::arm::client::DeploymentOp;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};

const TERMINAL_STATES: &[&str] = &["Succeeded", "Failed", "Canceled"];

pub struct Renderer {
    tty: bool,
    started: std::time::Instant,
    last_rendered_rows: usize,
    seen_states: HashMap<String, String>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            tty: std::io::stdout().is_terminal(),
            started: std::time::Instant::now(),
            last_rendered_rows: 0,
            seen_states: HashMap::new(),
        }
    }

    pub fn render(&mut self, ops: &[DeploymentOp]) {
        let rows: Vec<Row> = ops.iter().filter_map(Row::from_op).collect();
        if self.tty {
            self.render_tty(&rows);
        } else {
            self.render_stream(&rows);
        }
    }

    fn render_tty(&mut self, rows: &[Row]) {
        let mut out = std::io::stdout().lock();
        if self.last_rendered_rows > 0 {
            for _ in 0..self.last_rendered_rows {
                let _ = write!(out, "\x1b[1A\x1b[2K");
            }
        }
        let elapsed = self.started.elapsed().as_secs();
        let (done, total) = (
            rows.iter()
                .filter(|r| TERMINAL_STATES.contains(&r.state.as_str()))
                .count(),
            rows.len(),
        );
        let header = format!("==> deploy progress [T+{elapsed:>4}s] {done}/{total} ops");
        let _ = writeln!(out, "{header}");
        let w_state = rows.iter().map(|r| r.state.len()).max().unwrap_or(7).max(7);
        let w_type = rows
            .iter()
            .map(|r| r.resource_type.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let mut emitted = 1usize;
        for r in rows {
            let indent = "  ".repeat(r.depth as usize);
            let _ = writeln!(
                out,
                "  {indent}{:<ws$}  {:<wt$}  {}",
                r.state,
                r.resource_type,
                r.resource_name,
                ws = w_state,
                wt = w_type,
            );
            emitted += 1;
        }
        let _ = out.flush();
        self.last_rendered_rows = emitted;
    }

    fn render_stream(&mut self, rows: &[Row]) {
        let mut out = std::io::stdout().lock();
        let elapsed = self.started.elapsed().as_secs();
        for r in rows {
            let key = format!("{}/{}/{}", r.parent, r.resource_type, r.resource_name);
            let prev = self.seen_states.get(&key).cloned();
            if prev.as_deref() != Some(r.state.as_str()) {
                let indent = "  ".repeat(r.depth as usize);
                let _ = writeln!(
                    out,
                    "[T+{elapsed:>4}s] {indent}{:<10} {} {}",
                    r.state, r.resource_type, r.resource_name
                );
                self.seen_states.insert(key, r.state.clone());
            }
        }
        let _ = out.flush();
    }

    pub fn finish(&mut self) {
        if self.tty && self.last_rendered_rows > 0 {
            let _ = std::io::stdout().flush();
        }
    }
}

struct Row {
    state: String,
    resource_type: String,
    resource_name: String,
    depth: u8,
    parent: String,
}

impl Row {
    fn from_op(op: &DeploymentOp) -> Option<Self> {
        let props = op.op.get("properties")?;
        let state = props
            .get("provisioningState")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let target = props.get("targetResource")?;
        let resource_type = target
            .get("resourceType")
            .and_then(|v| v.as_str())
            .map(short_type)
            .unwrap_or_default();
        let resource_name = target
            .get("resourceName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if resource_name.is_empty() {
            return None;
        }
        Some(Self {
            state,
            resource_type,
            resource_name,
            depth: op.depth,
            parent: op.parent.clone(),
        })
    }
}

fn short_type(t: &str) -> String {
    t.rsplit('/').next().unwrap_or(t).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(parent: &str, depth: u8, state: &str, rtype: &str, rname: &str) -> DeploymentOp {
        DeploymentOp {
            parent: parent.to_string(),
            depth,
            op: json!({
                "properties": {
                    "provisioningState": state,
                    "targetResource": {
                        "resourceType": rtype,
                        "resourceName": rname,
                    }
                }
            }),
        }
    }

    #[test]
    fn row_extracts_state_type_name_depth_parent() {
        let r = Row::from_op(&op(
            "cluster-x",
            2,
            "Running",
            "Microsoft.Network/virtualNetworks",
            "vnet-demo",
        ))
        .unwrap();
        assert_eq!(r.state, "Running");
        assert_eq!(r.resource_type, "virtualNetworks");
        assert_eq!(r.resource_name, "vnet-demo");
        assert_eq!(r.depth, 2);
        assert_eq!(r.parent, "cluster-x");
    }

    #[test]
    fn row_filters_unnamed() {
        let d = DeploymentOp {
            parent: "x".into(),
            depth: 0,
            op: json!({"properties": {"provisioningState": "Running"}}),
        };
        assert!(Row::from_op(&d).is_none());
    }

    #[test]
    fn short_type_strips_provider_prefix() {
        assert_eq!(
            short_type("Microsoft.Compute/virtualMachineScaleSets"),
            "virtualMachineScaleSets"
        );
        assert_eq!(short_type("flat"), "flat");
    }
}
