use crate::types::{CheckOutcome, Runner};
use std::path::{Path, PathBuf};

const NVIDIA_VENDOR_ID: &str = "0x10de";

pub fn gpu_count(sys_root: &Path, dev_root: &Path) -> CheckOutcome {
    let name = "gpu_count";
    let mut pci_gpus = 0usize;
    let pci_devices = sys_root.join("bus/pci/devices");
    let entries = match std::fs::read_dir(&pci_devices) {
        Ok(it) => it,
        Err(e) => {
            return CheckOutcome::error(
                name,
                format!("cannot read {}: {e}", pci_devices.display()),
                vec![],
            );
        }
    };
    for ent in entries.flatten() {
        let vendor = std::fs::read_to_string(ent.path().join("vendor"))
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if vendor != NVIDIA_VENDOR_ID {
            continue;
        }
        let class = std::fs::read_to_string(ent.path().join("class"))
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if class.starts_with("0x0300") || class.starts_with("0x0302") {
            pci_gpus += 1;
        }
    }

    let mut dev_gpus = 0usize;
    if let Ok(it) = std::fs::read_dir(dev_root) {
        for ent in it.flatten() {
            if let Some(stem) = ent.file_name().to_str() {
                if let Some(suffix) = stem.strip_prefix("nvidia") {
                    if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                        dev_gpus += 1;
                    }
                }
            }
        }
    }

    if pci_gpus == 0 && dev_gpus == 0 {
        return CheckOutcome::ok(name, "no NVIDIA GPUs present (CPU node)");
    }
    if pci_gpus != dev_gpus {
        return CheckOutcome::error(
            name,
            format!("GPU count mismatch: PCI={pci_gpus} /dev={dev_gpus}"),
            vec![],
        );
    }
    CheckOutcome::ok(name, format!("{pci_gpus} GPUs visible"))
}

const XID_FATAL: &[u32] = &[48, 61, 62, 63, 64, 74, 79, 94, 95];
const XID_WARNING: &[u32] = &[43, 45];

pub fn gpu_xid(runner: &dyn Runner) -> CheckOutcome {
    let name = "gpu_xid";
    let out = match runner.run("dmesg", &["--time-format=iso"]) {
        Ok(o) => o,
        Err(e) => return CheckOutcome::error(name, format!("dmesg failed: {e}"), vec![]),
    };
    if !out.status.success() {
        return CheckOutcome::warning(
            name,
            format!("dmesg exited {}", out.status),
            vec![String::from_utf8_lossy(&out.stderr).trim().to_string()],
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut fatal: Vec<String> = vec![];
    let mut warns: Vec<String> = vec![];
    for line in stdout.lines() {
        let Some(idx) = line.find("NVRM: Xid") else {
            continue;
        };
        let tail = &line[idx..];
        let Some(rest) = tail.split_once("): ").map(|(_, r)| r) else {
            continue;
        };
        let xid: Option<u32> = rest
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok();
        let Some(xid) = xid else {
            continue;
        };
        let entry = format!("Xid {xid}: {}", line.trim());
        if XID_FATAL.contains(&xid) {
            fatal.push(entry);
        } else if XID_WARNING.contains(&xid) {
            warns.push(entry);
        } else {
            fatal.push(entry);
        }
    }
    if !fatal.is_empty() {
        return CheckOutcome::error(
            name,
            format!("{} fatal Xid event(s) in kernel log", fatal.len()),
            fatal,
        );
    }
    if !warns.is_empty() {
        return CheckOutcome::warning(
            name,
            format!("{} non-fatal Xid event(s) in kernel log", warns.len()),
            warns,
        );
    }
    CheckOutcome::ok(name, "no Xid events in kernel log")
}

pub fn network(sys_root: &Path) -> CheckOutcome {
    let name = "network";
    let net_dir = sys_root.join("class/net");
    let entries = match std::fs::read_dir(&net_dir) {
        Ok(it) => it,
        Err(e) => {
            return CheckOutcome::error(
                name,
                format!("cannot read {}: {e}", net_dir.display()),
                vec![],
            );
        }
    };

    let mut errors: Vec<String> = vec![];
    let mut warnings: Vec<String> = vec![];
    let mut checked: Vec<String> = vec![];
    for ent in entries.flatten() {
        let iface = ent.file_name().to_string_lossy().to_string();
        if iface == "lo" || iface.starts_with("docker") || iface.starts_with("veth") {
            continue;
        }
        let base = ent.path();
        let kind_raw = read_trim(&base.join("type")).unwrap_or_default();
        let kind = kind_raw.parse::<u32>().unwrap_or(0);
        if kind != 1 && kind != 32 {
            continue;
        }
        let operstate = read_trim(&base.join("operstate")).unwrap_or_else(|| "unknown".into());
        let carrier = read_trim(&base.join("carrier")).unwrap_or_default();
        checked.push(iface.clone());
        if operstate != "up" {
            errors.push(format!("{iface}: operstate={operstate}"));
        } else if carrier != "1" {
            errors.push(format!("{iface}: carrier=0"));
        }
        if let Some(c) =
            read_trim(&base.join("carrier_down_count")).and_then(|s| s.parse::<u64>().ok())
        {
            if c > 0 && operstate == "up" {
                warnings.push(format!("{iface}: carrier_down_count={c}"));
            }
        }
    }
    if !errors.is_empty() {
        return CheckOutcome::error(name, format!("{} interface(s) down", errors.len()), errors);
    }
    if !warnings.is_empty() {
        return CheckOutcome::warning(
            name,
            format!("{} interface(s) flapped", warnings.len()),
            warnings,
        );
    }
    if checked.is_empty() {
        return CheckOutcome::warning(name, "no Ethernet or InfiniBand interfaces found", vec![]);
    }
    CheckOutcome::ok(
        name,
        format!("{} interface(s) up: {}", checked.len(), checked.join(",")),
    )
}

pub fn kmsg(runner: &dyn Runner) -> CheckOutcome {
    let name = "kmsg";
    let out = match runner.run("dmesg", &["--level=emerg,alert,crit", "--since=1 hour ago"]) {
        Ok(o) => o,
        Err(e) => return CheckOutcome::error(name, format!("dmesg failed: {e}"), vec![]),
    };
    if !out.status.success() {
        return CheckOutcome::warning(
            name,
            format!("dmesg exited {}", out.status),
            vec![String::from_utf8_lossy(&out.stderr).trim().to_string()],
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<String> = stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if lines.is_empty() {
        return CheckOutcome::ok(name, "no critical kernel messages in last hour");
    }
    CheckOutcome::error(
        name,
        format!("{} critical kernel message(s) in last hour", lines.len()),
        lines,
    )
}

pub fn systemd(runner: &dyn Runner, services: &[String]) -> CheckOutcome {
    let name = "systemd";
    if services.is_empty() {
        return CheckOutcome::ok(name, "no services configured to check");
    }
    let mut failed: Vec<String> = vec![];
    let mut inactive: Vec<String> = vec![];
    let mut missing: Vec<String> = vec![];
    let mut active: Vec<String> = vec![];
    for svc in services {
        let out = runner.run("systemctl", &["is-active", svc]);
        let (status_str, code) = match out {
            Ok(o) => (
                String::from_utf8_lossy(&o.stdout).trim().to_string(),
                o.status.code().unwrap_or(-1),
            ),
            Err(e) => {
                missing.push(format!("{svc}: systemctl unavailable ({e})"));
                continue;
            }
        };
        match status_str.as_str() {
            "active" => active.push(svc.clone()),
            "failed" => failed.push(format!("{svc}: failed")),
            "inactive" | "activating" | "deactivating" | "reloading" => {
                inactive.push(format!("{svc}: {status_str}"));
            }
            "unknown" => missing.push(format!("{svc}: unit not found")),
            other => {
                if code == 4 {
                    missing.push(format!("{svc}: unit not found"));
                } else {
                    inactive.push(format!("{svc}: {other}"));
                }
            }
        }
    }
    if !failed.is_empty() {
        let mut all = failed.clone();
        all.extend(inactive);
        all.extend(missing);
        return CheckOutcome::error(name, format!("{} service(s) failed", failed.len()), all);
    }
    if !inactive.is_empty() {
        let mut all = inactive.clone();
        all.extend(missing);
        return CheckOutcome::warning(
            name,
            format!("{} service(s) not active", inactive.len()),
            all,
        );
    }
    CheckOutcome::ok(name, format!("{} service(s) active", active.len()))
}

fn read_trim(p: &PathBuf) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FakeRunner, Severity};
    use std::fs;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn gpu_count_no_gpus_is_ok() {
        let sys = tmpdir();
        let dev = tmpdir();
        fs::create_dir_all(sys.path().join("bus/pci/devices")).unwrap();
        let out = gpu_count(sys.path(), dev.path());
        assert_eq!(out.severity, Severity::Ok);
    }

    #[test]
    fn gpu_count_match_is_ok() {
        let sys = tmpdir();
        let dev = tmpdir();
        let devs = sys.path().join("bus/pci/devices");
        fs::create_dir_all(&devs).unwrap();
        for i in 0..2 {
            let d = devs.join(format!("0000:00:0{i}.0"));
            fs::create_dir(&d).unwrap();
            fs::write(d.join("vendor"), "0x10de\n").unwrap();
            fs::write(d.join("class"), "0x030200\n").unwrap();
            fs::write(dev.path().join(format!("nvidia{i}")), "").unwrap();
        }
        let out = gpu_count(sys.path(), dev.path());
        assert_eq!(out.severity, Severity::Ok, "{}", out.message);
    }

    #[test]
    fn gpu_count_mismatch_is_error() {
        let sys = tmpdir();
        let dev = tmpdir();
        let devs = sys.path().join("bus/pci/devices");
        fs::create_dir_all(&devs).unwrap();
        let d = devs.join("0000:00:00.0");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("vendor"), "0x10de\n").unwrap();
        fs::write(d.join("class"), "0x030200\n").unwrap();
        let out = gpu_count(sys.path(), dev.path());
        assert_eq!(out.severity, Severity::Error);
    }

    #[test]
    fn gpu_xid_clean() {
        let r = FakeRunner::new().with("dmesg --time-format=iso", "boot ok\n", 0);
        let out = gpu_xid(&r);
        assert_eq!(out.severity, Severity::Ok);
    }

    #[test]
    fn gpu_xid_fatal() {
        let r = FakeRunner::new().with(
            "dmesg --time-format=iso",
            "2026-01-01T00:00:00 NVRM: Xid (PCI:0000:01:00): 79, pid=0\n",
            0,
        );
        let out = gpu_xid(&r);
        assert_eq!(out.severity, Severity::Error);
        assert!(out.message.contains("1 fatal"));
    }

    #[test]
    fn gpu_xid_warning() {
        let r = FakeRunner::new().with(
            "dmesg --time-format=iso",
            "2026-01-01 NVRM: Xid (PCI:0000:01:00): 43, ch 00\n",
            0,
        );
        let out = gpu_xid(&r);
        assert_eq!(out.severity, Severity::Warning);
    }

    #[test]
    fn network_eth_down_is_error() {
        let sys = tmpdir();
        let net = sys.path().join("class/net/eth0");
        fs::create_dir_all(&net).unwrap();
        fs::write(net.join("type"), "1\n").unwrap();
        fs::write(net.join("operstate"), "down\n").unwrap();
        fs::write(net.join("carrier"), "0\n").unwrap();
        let out = network(sys.path());
        assert_eq!(out.severity, Severity::Error);
    }

    #[test]
    fn network_ib_up_is_ok() {
        let sys = tmpdir();
        let net = sys.path().join("class/net/mlx5_ib0");
        fs::create_dir_all(&net).unwrap();
        fs::write(net.join("type"), "32\n").unwrap();
        fs::write(net.join("operstate"), "up\n").unwrap();
        fs::write(net.join("carrier"), "1\n").unwrap();
        let out = network(sys.path());
        assert_eq!(out.severity, Severity::Ok, "{}", out.message);
    }

    #[test]
    fn network_flap_is_warning() {
        let sys = tmpdir();
        let net = sys.path().join("class/net/eth0");
        fs::create_dir_all(&net).unwrap();
        fs::write(net.join("type"), "1\n").unwrap();
        fs::write(net.join("operstate"), "up\n").unwrap();
        fs::write(net.join("carrier"), "1\n").unwrap();
        fs::write(net.join("carrier_down_count"), "3\n").unwrap();
        let out = network(sys.path());
        assert_eq!(out.severity, Severity::Warning);
    }

    #[test]
    fn kmsg_clean() {
        let r = FakeRunner::new().with("dmesg --level=emerg,alert,crit --since=1 hour ago", "", 0);
        let out = kmsg(&r);
        assert_eq!(out.severity, Severity::Ok);
    }

    #[test]
    fn kmsg_critical() {
        let r = FakeRunner::new().with(
            "dmesg --level=emerg,alert,crit --since=1 hour ago",
            "kernel panic - not syncing\n",
            0,
        );
        let out = kmsg(&r);
        assert_eq!(out.severity, Severity::Error);
    }

    #[test]
    fn systemd_all_active() {
        let r = FakeRunner::new()
            .with("systemctl is-active slurmd", "active\n", 0)
            .with("systemctl is-active prometheus", "active\n", 0);
        let out = systemd(&r, &["slurmd".into(), "prometheus".into()]);
        assert_eq!(out.severity, Severity::Ok);
    }

    #[test]
    fn systemd_failed_is_error() {
        let r = FakeRunner::new()
            .with("systemctl is-active slurmd", "failed\n", 3)
            .with("systemctl is-active prometheus", "active\n", 0);
        let out = systemd(&r, &["slurmd".into(), "prometheus".into()]);
        assert_eq!(out.severity, Severity::Error);
    }

    #[test]
    fn systemd_inactive_is_warning() {
        let r = FakeRunner::new().with("systemctl is-active slurmd", "inactive\n", 3);
        let out = systemd(&r, &["slurmd".into()]);
        assert_eq!(out.severity, Severity::Warning);
    }
}
