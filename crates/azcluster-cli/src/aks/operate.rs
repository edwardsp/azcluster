pub(crate) fn split_ns_pod(s: &str) -> (String, String) {
    match s.split_once('/') {
        Some((ns, pod)) => (ns.to_string(), pod.to_string()),
        None => ("default".to_string(), s.to_string()),
    }
}

pub(crate) fn kubectl_exec_args(
    ns: &str,
    pod: &str,
    container: Option<&str>,
    cmd: &[String],
) -> Vec<String> {
    let mut a = vec!["exec".to_string(), "-n".to_string(), ns.to_string()];
    if let Some(c) = container {
        a.push("-c".to_string());
        a.push(c.to_string());
    }
    a.push(pod.to_string());
    a.push("--".to_string());
    a.extend(cmd.iter().cloned());
    a
}

pub(crate) fn kubectl_logs_args(
    ns: &str,
    pod: &str,
    container: Option<&str>,
    tail: u32,
    follow: bool,
) -> Vec<String> {
    let mut a = vec!["logs".to_string(), "-n".to_string(), ns.to_string()];
    if let Some(c) = container {
        a.push("-c".to_string());
        a.push(c.to_string());
    }
    a.push(pod.to_string());
    if tail == 0 {
        a.push("--tail=-1".to_string());
    } else {
        a.push(format!("--tail={tail}"));
    }
    if follow {
        a.push("-f".to_string());
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_defaults_to_default_namespace() {
        assert_eq!(split_ns_pod("mypod"), ("default".into(), "mypod".into()));
        assert_eq!(
            split_ns_pod("kube-system/coredns"),
            ("kube-system".into(), "coredns".into())
        );
    }

    #[test]
    fn exec_args_wrap_command_after_dashes() {
        let a = kubectl_exec_args(
            "default",
            "p0",
            None,
            &["bash".into(), "-c".into(), "ls /".into()],
        );
        assert_eq!(
            a,
            vec!["exec", "-n", "default", "p0", "--", "bash", "-c", "ls /"]
        );
        let a2 = kubectl_exec_args("default", "p0", Some("sglang"), &["nvidia-smi".into()]);
        assert_eq!(
            a2,
            vec![
                "exec",
                "-n",
                "default",
                "-c",
                "sglang",
                "p0",
                "--",
                "nvidia-smi"
            ]
        );
    }

    #[test]
    fn logs_args_tail_and_follow() {
        assert_eq!(
            kubectl_logs_args("default", "p0", None, 50, false),
            vec!["logs", "-n", "default", "p0", "--tail=50"]
        );
        assert_eq!(
            kubectl_logs_args("kube-system", "p0", Some("c"), 0, true),
            vec![
                "logs",
                "-n",
                "kube-system",
                "-c",
                "c",
                "p0",
                "--tail=-1",
                "-f"
            ]
        );
    }
}
