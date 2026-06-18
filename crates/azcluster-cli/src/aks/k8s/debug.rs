use anyhow::{bail, Result};
use serde_json::json;
use std::time::{Duration, Instant};

use super::client::K8sClient;

const DEBUG_IMAGE: &str = "mcr.microsoft.com/cbl-mariner/busybox:2.0";

pub(crate) fn node_shell(client: &K8sClient, node: &str) -> Result<()> {
    let pod = format!("azcluster-debug-{}-{}", sanitize_node(node), short_random());
    let body = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": pod, "namespace": "default" },
        "spec": {
            "nodeName": node,
            "hostPID": true,
            "hostNetwork": true,
            "hostIPC": true,
            "restartPolicy": "Never",
            "tolerations": [{ "operator": "Exists" }],
            "volumes": [{ "name": "host", "hostPath": { "path": "/", "type": "Directory" } }],
            "containers": [{
                "name": "debugger",
                "image": DEBUG_IMAGE,
                "securityContext": { "privileged": true },
                "stdin": true,
                "tty": true,
                "volumeMounts": [{ "name": "host", "mountPath": "/host" }],
                "command": ["chroot", "/host", "bash"]
            }]
        }
    });
    client.api_post_json("/api/v1/namespaces/default/pods", &body)?;
    let cleanup = DebugPodCleanup {
        client,
        pod: pod.clone(),
    };
    wait_running(client, &pod)?;
    let code = super::exec::attach(client, "default", &pod, Some("debugger"), true)?;
    drop(cleanup);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn wait_running(client: &K8sClient, pod: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let v = client.api_get_json(&format!("/api/v1/namespaces/default/pods/{pod}"))?;
        let phase = v
            .get("status")
            .and_then(|s| s.get("phase"))
            .and_then(|p| p.as_str())
            .unwrap_or("");
        match phase {
            "Running" => return Ok(()),
            "Failed" | "Succeeded" => bail!("debug pod {pod} reached terminal phase {phase}"),
            _ if Instant::now() >= deadline => {
                bail!("debug pod {pod} did not reach Running within 60s")
            }
            _ => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

struct DebugPodCleanup<'a> {
    client: &'a K8sClient,
    pod: String,
}

impl Drop for DebugPodCleanup<'_> {
    fn drop(&mut self) {
        let path = format!(
            "/api/v1/namespaces/default/pods/{}?gracePeriodSeconds=0",
            self.pod
        );
        if let Err(e) = self.client.api_delete(&path) {
            eprintln!("warning: failed to delete debug pod {}: {e:#}", self.pod);
        }
    }
}

fn sanitize_node(node: &str) -> String {
    let mut out: String = node
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    out.truncate(40);
    out.trim_matches('-').to_string()
}

fn short_random() -> String {
    use rand::RngCore;
    let mut b = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut b);
    format!("{:08x}", u32::from_be_bytes(b))
}
