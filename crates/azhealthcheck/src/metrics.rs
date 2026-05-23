use crate::types::{CheckOutcome, Severity};
use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Filename written into `--metrics-dir`. node_exporter's textfile collector
/// reads every `*.prom` file in its directory; using a stable name lets the
/// collector replace stale series automatically when we rewrite.
pub const METRICS_FILENAME: &str = "azhealthcheck.prom";

/// Render Prometheus exposition text for the given outcomes.
///
/// Emits four series, labelled by `check` (and `host` on the per-check ones):
/// - `azcluster_healthcheck_severity{check,host}`          0=ok, 1=warning, 2=error
/// - `azcluster_healthcheck_findings_total{check,host}`    finding count per check
/// - `azcluster_healthcheck_worst_severity{host}`          max severity across all checks
/// - `azcluster_healthcheck_last_run_timestamp_seconds{host}`  unix time of this run
///
/// `host` is a stable label so Grafana can dedupe across `instance` reshuffles
/// (node_exporter sets `instance` from the scrape target, which the textfile
/// collector inherits).
pub fn render(hostname: &str, outcomes: &[CheckOutcome], now_unix_seconds: u64) -> String {
    let host = escape_label(hostname);
    let mut out = String::with_capacity(1024);

    out.push_str(
        "# HELP azcluster_healthcheck_severity Severity per check (0=ok, 1=warning, 2=error).\n",
    );
    out.push_str("# TYPE azcluster_healthcheck_severity gauge\n");
    for o in outcomes {
        let check = escape_label(o.name);
        let sev = severity_value(o.severity);
        out.push_str(&format!(
            "azcluster_healthcheck_severity{{check=\"{check}\",host=\"{host}\"}} {sev}\n"
        ));
    }

    out.push_str("# HELP azcluster_healthcheck_findings_total Number of findings emitted by each check on this run.\n");
    out.push_str("# TYPE azcluster_healthcheck_findings_total gauge\n");
    for o in outcomes {
        let check = escape_label(o.name);
        let n = o.findings.len();
        out.push_str(&format!(
            "azcluster_healthcheck_findings_total{{check=\"{check}\",host=\"{host}\"}} {n}\n"
        ));
    }

    let worst = outcomes
        .iter()
        .map(|o| o.severity)
        .max()
        .unwrap_or(Severity::Ok);
    out.push_str(
        "# HELP azcluster_healthcheck_worst_severity Worst severity across all checks on this run.\n",
    );
    out.push_str("# TYPE azcluster_healthcheck_worst_severity gauge\n");
    out.push_str(&format!(
        "azcluster_healthcheck_worst_severity{{host=\"{host}\"}} {}\n",
        severity_value(worst)
    ));

    out.push_str(
        "# HELP azcluster_healthcheck_last_run_timestamp_seconds Unix time of the most recent azhealthcheck run.\n",
    );
    out.push_str("# TYPE azcluster_healthcheck_last_run_timestamp_seconds gauge\n");
    out.push_str(&format!(
        "azcluster_healthcheck_last_run_timestamp_seconds{{host=\"{host}\"}} {now_unix_seconds}\n"
    ));

    out
}

/// Atomically write the rendered prom file into `dir`.
///
/// node_exporter's textfile collector skips files that contain a `.` in their
/// basename other than the trailing `.prom` extension, and on some versions
/// will emit a `node_textfile_scrape_error` for partial files. Writing to a
/// sibling `.tmp.<pid>` and `rename(2)`-ing onto the final path side-steps
/// both issues: the collector either sees the previous complete file or the
/// new complete file, never a torn write.
pub fn write_atomic(dir: &Path, contents: &str) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create metrics dir {}", dir.display()))?;
    let tmp = dir.join(format!(".{METRICS_FILENAME}.tmp.{}", std::process::id()));
    let final_path = dir.join(METRICS_FILENAME);
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("open temp metrics file {}", tmp.display()))?;
        f.write_all(contents.as_bytes())
            .with_context(|| format!("write temp metrics file {}", tmp.display()))?;
        f.sync_all().ok();
    }
    // 0644 so the unprivileged node_exporter user can read it; create()
    // defaults to 0600 which would mask the file from the scrape.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
        .with_context(|| format!("chmod temp metrics file {}", tmp.display()))?;
    std::fs::rename(&tmp, &final_path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), final_path.display()))?;
    Ok(())
}

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn severity_value(s: Severity) -> u8 {
    match s {
        Severity::Ok => 0,
        Severity::Warning => 1,
        Severity::Error => 2,
    }
}

fn escape_label(s: &str) -> String {
    // Per Prometheus exposition format: escape `\`, `"`, and `\n` in label values.
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CheckOutcome;

    #[test]
    fn render_contains_all_series_and_labels() {
        let outcomes = vec![
            CheckOutcome::ok("gpu_count", "8 GPUs visible"),
            CheckOutcome::warning("kmsg", "1 critical message", vec!["oom-killer".to_string()]),
            CheckOutcome::error(
                "systemd",
                "slurmd inactive",
                vec!["slurmd: inactive".to_string(), "extra".to_string()],
            ),
        ];
        let text = render("compute01", &outcomes, 1_700_000_000);
        assert!(text
            .contains("azcluster_healthcheck_severity{check=\"gpu_count\",host=\"compute01\"} 0"));
        assert!(
            text.contains("azcluster_healthcheck_severity{check=\"kmsg\",host=\"compute01\"} 1")
        );
        assert!(
            text.contains("azcluster_healthcheck_severity{check=\"systemd\",host=\"compute01\"} 2")
        );
        assert!(text.contains(
            "azcluster_healthcheck_findings_total{check=\"gpu_count\",host=\"compute01\"} 0"
        ));
        assert!(text
            .contains("azcluster_healthcheck_findings_total{check=\"kmsg\",host=\"compute01\"} 1"));
        assert!(text.contains(
            "azcluster_healthcheck_findings_total{check=\"systemd\",host=\"compute01\"} 2"
        ));
        assert!(text.contains("azcluster_healthcheck_worst_severity{host=\"compute01\"} 2"));
        assert!(text.contains(
            "azcluster_healthcheck_last_run_timestamp_seconds{host=\"compute01\"} 1700000000"
        ));
        assert_eq!(
            text.matches("# TYPE ").count(),
            4,
            "expected 4 TYPE lines, got:\n{text}"
        );
    }

    #[test]
    fn render_empty_outcomes_still_emits_worst_zero_and_timestamp() {
        let text = render("h", &[], 42);
        assert!(text.contains("azcluster_healthcheck_worst_severity{host=\"h\"} 0"));
        assert!(text.contains("azcluster_healthcheck_last_run_timestamp_seconds{host=\"h\"} 42"));
        assert!(!text.contains("azcluster_healthcheck_severity{check="));
    }

    #[test]
    fn render_escapes_quotes_and_backslashes_in_host() {
        let text = render("weird\"host\\name", &[], 0);
        assert!(text.contains("host=\"weird\\\"host\\\\name\""));
    }

    #[test]
    fn write_atomic_creates_file_with_mode_0644_and_replaces_existing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_atomic(dir.path(), "first\n").unwrap();
        let final_path = dir.path().join(METRICS_FILENAME);
        assert_eq!(std::fs::read_to_string(&final_path).unwrap(), "first\n");
        let mode = std::fs::metadata(&final_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "expected 0644, got {mode:o}");
        // Overwrite with new contents.
        write_atomic(dir.path(), "second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&final_path).unwrap(), "second\n");
        write_atomic(dir.path(), "second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&final_path).unwrap(), "second\n");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != std::ffi::OsStr::new(METRICS_FILENAME))
            .collect();
        assert!(
            leftovers.is_empty(),
            "unexpected files: {:?}",
            leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn write_atomic_creates_missing_dir() {
        let parent = tempfile::tempdir().unwrap();
        let nested = parent.path().join("a/b/c");
        write_atomic(&nested, "x").unwrap();
        assert!(nested.join(METRICS_FILENAME).is_file());
    }
}
