use anyhow::{bail, Context, Result};
use std::io::{Read, Write};

use super::client::K8sClient;

pub(crate) fn run(
    client: &K8sClient,
    ns: &str,
    pod: &str,
    container: Option<&str>,
    tail: u32,
    follow: bool,
) -> Result<()> {
    let mut url = client.url(&format!("/api/v1/namespaces/{ns}/pods/{pod}/log"))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("follow", if follow { "true" } else { "false" });
        if tail > 0 {
            qp.append_pair("tailLines", &tail.to_string());
        }
        if let Some(c) = container {
            qp.append_pair("container", c);
        }
    }
    let mut resp = client
        .http
        .get(url.clone())
        .send()
        .context("GET pod logs")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        bail!("Kubernetes logs GET failed ({status}): {body}");
    }
    let mut stdout = std::io::stdout();
    if follow {
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = resp.read(&mut buf).context("read log stream")?;
            if n == 0 {
                break;
            }
            stdout.write_all(&buf[..n]).context("write log stream")?;
            stdout.flush().ok();
        }
    } else {
        resp.copy_to(&mut stdout).context("copy pod logs")?;
    }
    Ok(())
}
