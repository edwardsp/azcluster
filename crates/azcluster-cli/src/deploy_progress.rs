use serde_json::Value;
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

    pub fn render(&mut self, ops: &[Value]) {
        let mut rows: Vec<Row> = ops.iter().filter_map(Row::from_op).collect();
        rows.sort_by(|a, b| {
            terminal_rank(&a.state)
                .cmp(&terminal_rank(&b.state))
                .then(a.resource_name.cmp(&b.resource_name))
        });

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
            let _ = writeln!(
                out,
                "  {:<ws$}  {:<wt$}  {}",
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
            let key = format!("{}/{}", r.resource_type, r.resource_name);
            let prev = self.seen_states.get(&key).cloned();
            if prev.as_deref() != Some(r.state.as_str()) {
                let _ = writeln!(
                    out,
                    "[T+{elapsed:>4}s] {:<10} {} {}",
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

fn terminal_rank(state: &str) -> u8 {
    match state {
        "Failed" | "Canceled" => 0,
        "Running" => 1,
        "Accepted" => 2,
        "Succeeded" => 3,
        _ => 4,
    }
}

struct Row {
    state: String,
    resource_type: String,
    resource_name: String,
}

impl Row {
    fn from_op(op: &Value) -> Option<Self> {
        let props = op.get("properties")?;
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

    #[test]
    fn row_extracts_state_type_name() {
        let op = json!({
            "properties": {
                "provisioningState": "Running",
                "targetResource": {
                    "resourceType": "Microsoft.Network/virtualNetworks",
                    "resourceName": "vnet-demo"
                }
            }
        });
        let r = Row::from_op(&op).unwrap();
        assert_eq!(r.state, "Running");
        assert_eq!(r.resource_type, "virtualNetworks");
        assert_eq!(r.resource_name, "vnet-demo");
    }

    #[test]
    fn row_filters_unnamed() {
        let op = json!({"properties": {"provisioningState": "Running"}});
        assert!(Row::from_op(&op).is_none());
    }

    #[test]
    fn terminal_rank_orders_failed_first_then_running_then_done() {
        assert!(terminal_rank("Failed") < terminal_rank("Running"));
        assert!(terminal_rank("Running") < terminal_rank("Succeeded"));
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
