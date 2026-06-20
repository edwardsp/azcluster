pub(crate) fn split_ns_pod(s: &str) -> (String, String) {
    match s.split_once('/') {
        Some((ns, pod)) => (ns.to_string(), pod.to_string()),
        None => ("default".to_string(), s.to_string()),
    }
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
}
