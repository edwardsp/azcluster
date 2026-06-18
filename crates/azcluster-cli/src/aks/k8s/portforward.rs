use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use super::client::{block_on, K8sClient};
use super::exec::{connect_ws, frame, split_frame};

pub(crate) const DATA_CHANNEL: u8 = 0;
pub(crate) const ERROR_CHANNEL: u8 = 1;

pub(crate) fn run(client: &K8sClient, ns: &str, pod: &str, spec: &str) -> Result<()> {
    let (local_port, remote_port) = parse_port_spec(spec)?;
    let c = client.clone();
    let ns = ns.to_string();
    let pod = pod.to_string();
    block_on(async move { run_async(c, ns, pod, local_port, remote_port).await })
}

fn parse_port_spec(spec: &str) -> Result<(u16, u16)> {
    let (local, remote) = spec
        .split_once(':')
        .ok_or_else(|| anyhow!("AKS tunnel port must be local:remote"))?;
    Ok((
        local.parse().context("parse local port")?,
        remote.parse().context("parse remote port")?,
    ))
}

async fn run_async(
    client: K8sClient,
    ns: String,
    pod: String,
    local_port: u16,
    remote_port: u16,
) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", local_port))
        .await
        .with_context(|| format!("bind 127.0.0.1:{local_port}"))?;
    eprintln!("==> forwarding 127.0.0.1:{local_port} -> {ns}/{pod}:{remote_port} (Ctrl-C to stop)");
    loop {
        let (tcp, peer) = listener
            .accept()
            .await
            .context("accept local port-forward client")?;
        let c = client.clone();
        let ns = ns.clone();
        let pod = pod.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_one(&c, &ns, &pod, remote_port, tcp).await {
                eprintln!("port-forward client {peer} failed: {e:#}");
            }
        });
    }
}

async fn handle_one(
    client: &K8sClient,
    ns: &str,
    pod: &str,
    remote_port: u16,
    tcp: TcpStream,
) -> Result<()> {
    let mut url = client.wss_url(&format!("/api/v1/namespaces/{ns}/pods/{pod}/portforward"))?;
    url.query_pairs_mut()
        .append_pair("ports", &remote_port.to_string());
    let mut ws = connect_ws(client, url.as_str(), "portforward.k8s.io").await?;
    read_server_port_frame(&mut ws, DATA_CHANNEL, remote_port).await?;
    read_server_port_frame(&mut ws, ERROR_CHANNEL, remote_port).await?;

    let (mut ws_write, mut ws_read) = ws.split();
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let to_ws = async move {
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = tcp_read
                .read(&mut buf)
                .await
                .context("read local port-forward client")?;
            if n == 0 {
                break;
            }
            ws_write
                .send(Message::Binary(frame(DATA_CHANNEL, &buf[..n])))
                .await
                .context("send port-forward data")?;
        }
        let _ = ws_write.close().await;
        Ok::<(), anyhow::Error>(())
    };
    let from_ws = async move {
        while let Some(msg) = ws_read.next().await {
            match msg.context("read port-forward WebSocket")? {
                Message::Binary(data) => {
                    let (ch, payload) = split_frame(&data)?;
                    match ch {
                        DATA_CHANNEL => tcp_write
                            .write_all(payload)
                            .await
                            .context("write local port-forward client")?,
                        ERROR_CHANNEL => bail!(
                            "Kubernetes port-forward error: {}",
                            String::from_utf8_lossy(payload)
                        ),
                        _ => {}
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = tcp_write.shutdown().await;
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        r = to_ws => r,
        r = from_ws => r,
    }
}

async fn read_server_port_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>,
    expected_channel: u8,
    expected_port: u16,
) -> Result<()> {
    let msg = ws
        .next()
        .await
        .ok_or_else(|| anyhow!("Kubernetes port-forward closed before port frame"))?
        .context("read Kubernetes port-forward port frame")?;
    let Message::Binary(data) = msg else {
        bail!("Kubernetes port-forward expected binary port frame");
    };
    let port = parse_server_port_frame(&data, expected_channel)?;
    if port != expected_port {
        bail!("Kubernetes port-forward server selected port {port}, expected {expected_port}");
    }
    Ok(())
}

fn parse_server_port_frame(data: &[u8], expected_channel: u8) -> Result<u16> {
    let (ch, payload) = split_frame(data)?;
    if ch != expected_channel || payload.len() != 2 {
        bail!("Kubernetes port-forward invalid port frame");
    }
    Ok(u16::from_le_bytes([payload[0], payload[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_local_remote_pair() {
        assert_eq!(parse_port_spec("18080:8080").unwrap(), (18080, 8080));
        assert!(parse_port_spec("8080").is_err());
    }

    #[test]
    fn parses_server_port_frame_little_endian() {
        assert_eq!(
            parse_server_port_frame(&[DATA_CHANNEL, 0x90, 0x1f], DATA_CHANNEL).unwrap(),
            8080
        );
        assert_eq!(
            parse_server_port_frame(&[ERROR_CHANNEL, 0x90, 0x1f], ERROR_CHANNEL).unwrap(),
            8080
        );
        assert!(parse_server_port_frame(&[ERROR_CHANNEL, 0x90, 0x1f], DATA_CHANNEL).is_err());
    }
}
