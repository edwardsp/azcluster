use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::io::Write;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http, Message};

use super::client::{block_on, K8sClient};

pub(crate) const CH_STDIN: u8 = 0;
pub(crate) const CH_STDOUT: u8 = 1;
pub(crate) const CH_STDERR: u8 = 2;
pub(crate) const CH_ERROR: u8 = 3;
pub(crate) const CH_RESIZE: u8 = 4;

pub(crate) fn frame(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.push(channel);
    out.extend_from_slice(payload);
    out
}

pub(crate) fn split_frame(data: &[u8]) -> Result<(u8, &[u8])> {
    let (&ch, rest) = data
        .split_first()
        .ok_or_else(|| anyhow!("empty Kubernetes channel frame"))?;
    Ok((ch, rest))
}

pub(crate) fn run_for_exit_code(
    client: &K8sClient,
    ns: &str,
    pod: &str,
    container: Option<&str>,
    cmd: &[String],
    tty: bool,
) -> Result<i32> {
    let mut url = client.wss_url(&format!("/api/v1/namespaces/{ns}/pods/{pod}/exec"))?;
    {
        let mut q = url.query_pairs_mut();
        for arg in cmd {
            q.append_pair("command", arg);
        }
        q.append_pair("stdin", "true");
        q.append_pair("stdout", "true");
        q.append_pair("stderr", "true");
        q.append_pair("tty", if tty { "true" } else { "false" });
        if let Some(c) = container {
            q.append_pair("container", c);
        }
    }
    attach_url(client, url.as_str(), tty)
}

pub(crate) fn attach(
    client: &K8sClient,
    ns: &str,
    pod: &str,
    container: Option<&str>,
    tty: bool,
) -> Result<i32> {
    let mut url = client.wss_url(&format!("/api/v1/namespaces/{ns}/pods/{pod}/attach"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("stdin", "true");
        q.append_pair("stdout", "true");
        q.append_pair("stderr", "true");
        q.append_pair("tty", if tty { "true" } else { "false" });
        if let Some(c) = container {
            q.append_pair("container", c);
        }
    }
    attach_url(client, url.as_str(), tty)
}

fn attach_url(client: &K8sClient, url: &str, tty: bool) -> Result<i32> {
    let _raw = if tty { Some(RawMode::enter()?) } else { None };
    let c = client.clone();
    block_on(async move { attach_loop(&c, url.to_string(), tty).await })
}

async fn attach_loop(client: &K8sClient, url: String, tty: bool) -> Result<i32> {
    let mut ws = connect_ws(client, &url, "v5.channel.k8s.io").await?;

    if tty {
        if let Some((w, h)) = terminal_size() {
            let payload = serde_json::json!({ "Width": w, "Height": h }).to_string();
            ws.send(Message::Binary(frame(CH_RESIZE, payload.as_bytes())))
                .await
                .context("send initial terminal resize")?;
        }
    }

    let (mut ws_write, mut ws_read) = ws.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
    let writer_task = tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            ws_write
                .send(Message::Binary(data))
                .await
                .context("send Kubernetes exec frame")?;
        }
        Ok::<(), anyhow::Error>(())
    });
    let stdin_tx = tx.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = stdin.read(&mut buf).await.context("read stdin")?;
            if n == 0 {
                break;
            }
            stdin_tx
                .send(frame(CH_STDIN, &buf[..n]))
                .await
                .context("send stdin")?;
        }
        Ok::<(), anyhow::Error>(())
    });
    let resize_task = spawn_resize_task(tty, tx.clone());
    drop(tx);

    let mut error_payload = Vec::new();
    while let Some(msg) = ws_read.next().await {
        match msg.context("read Kubernetes exec WebSocket")? {
            Message::Binary(data) => {
                let (ch, payload) = split_frame(&data)?;
                match ch {
                    CH_STDOUT => std::io::stdout()
                        .write_all(payload)
                        .context("write stdout")?,
                    CH_STDERR => std::io::stderr()
                        .write_all(payload)
                        .context("write stderr")?,
                    CH_ERROR => {
                        // ch3 carries the terminal metav1.Status; the server may not
                        // send a WS Close in tty mode, so this frame ends the session.
                        error_payload.extend_from_slice(payload);
                        break;
                    }
                    _ => {}
                }
            }
            Message::Text(text) => eprint!("{text}"),
            Message::Close(_) => break,
            _ => {}
        }
    }
    stdin_task.abort();
    if let Some(t) = resize_task {
        t.abort();
    }
    writer_task.abort();
    std::io::stdout().flush().ok();
    std::io::stderr().flush().ok();
    parse_exit_code(&error_payload)
}

#[cfg(unix)]
fn spawn_resize_task(tty: bool, tx: mpsc::Sender<Vec<u8>>) -> Option<tokio::task::JoinHandle<()>> {
    if !tty {
        return None;
    }
    Some(tokio::spawn(async move {
        let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())
        else {
            return;
        };
        while sig.recv().await.is_some() {
            if let Some((w, h)) = terminal_size() {
                let payload = serde_json::json!({ "Width": w, "Height": h }).to_string();
                if tx.send(frame(CH_RESIZE, payload.as_bytes())).await.is_err() {
                    break;
                }
            }
        }
    }))
}

#[cfg(not(unix))]
fn spawn_resize_task(
    _tty: bool,
    _tx: mpsc::Sender<Vec<u8>>,
) -> Option<tokio::task::JoinHandle<()>> {
    None
}

pub(crate) fn ws_request(url: &str, protocol: &str) -> Result<http::Request<()>> {
    let mut req = url
        .into_client_request()
        .context("build WebSocket request")?;
    req.headers_mut().insert(
        http::header::SEC_WEBSOCKET_PROTOCOL,
        http::HeaderValue::from_str(protocol)?,
    );
    Ok(req)
}

pub(crate) async fn connect_ws(
    client: &K8sClient,
    url: &str,
    protocol: &str,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>> {
    let tcp = TcpStream::connect((client.server_host.as_str(), client.server_port))
        .await
        .with_context(|| format!("connect {}:{}", client.server_host, client.server_port))?;
    tcp.set_nodelay(true)?;
    let server_name = rustls::pki_types::ServerName::try_from(client.server_host.clone())
        .context("invalid Kubernetes TLS server name")?;
    let tls = TlsConnector::from(client.tls.clone())
        .connect(server_name, tcp)
        .await
        .context("Kubernetes TLS handshake")?;
    let req = ws_request(url, protocol)?;
    let (ws, _) = tokio_tungstenite::client_async(req, tls)
        .await
        .context("Kubernetes WebSocket upgrade")?;
    Ok(ws)
}

#[derive(Deserialize)]
struct Status {
    #[serde(default)]
    status: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    details: Option<StatusDetails>,
}

#[derive(Deserialize)]
struct StatusDetails {
    #[serde(default)]
    causes: Vec<StatusCause>,
}

#[derive(Deserialize)]
struct StatusCause {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    message: String,
}

pub(crate) fn parse_exit_code(payload: &[u8]) -> Result<i32> {
    if payload.is_empty() {
        return Ok(0);
    }
    let s: Status = serde_json::from_slice(payload).context("parse Kubernetes exec status")?;
    if s.status != "Failure" {
        return Ok(0);
    }
    if s.reason != "NonZeroExitCode"
        && !payload
            .windows(b"NonZeroExitCode".len())
            .any(|w| w == b"NonZeroExitCode")
    {
        bail!(
            "Kubernetes exec failed: {}",
            String::from_utf8_lossy(payload)
        );
    }
    if let Some(details) = s.details {
        for c in details.causes {
            if c.reason == "ExitCode" {
                return c
                    .message
                    .parse::<i32>()
                    .context("parse Kubernetes exec exit code");
            }
        }
    }
    Ok(1)
}

#[cfg(unix)]
struct RawMode {
    fd: libc::c_int,
    original: libc::termios,
}

#[cfg(unix)]
impl RawMode {
    fn enter() -> Result<Self> {
        let fd = libc::STDIN_FILENO;
        let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, term.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error()).context("read terminal attributes");
        }
        let original = unsafe { term.assume_init() };
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(std::io::Error::last_os_error()).context("set raw terminal mode");
        }
        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
    }
}

#[cfg(not(unix))]
struct RawMode;

#[cfg(not(unix))]
impl RawMode {
    fn enter() -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(unix)]
fn terminal_size() -> Option<(u16, u16)> {
    let mut ws = std::mem::MaybeUninit::<libc::winsize>::uninit();
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, ws.as_mut_ptr()) } == 0 {
        let ws = unsafe { ws.assume_init() };
        if ws.ws_col > 0 && ws.ws_row > 0 {
            return Some((ws.ws_col, ws.ws_row));
        }
    }
    None
}

#[cfg(not(unix))]
fn terminal_size() -> Option<(u16, u16)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_channel_frame_encode_decode() {
        let encoded = frame(CH_STDOUT, b"hello");
        assert_eq!(encoded, b"\x01hello");
        let (ch, payload) = split_frame(&encoded).unwrap();
        assert_eq!(ch, CH_STDOUT);
        assert_eq!(payload, b"hello");
        assert!(split_frame(&[]).is_err());
    }

    #[test]
    fn exec_status_exit_code_is_parsed() {
        let payload = br#"{"status":"Failure","reason":"NonZeroExitCode","details":{"causes":[{"reason":"ExitCode","message":"7"}]}}"#;
        assert_eq!(parse_exit_code(payload).unwrap(), 7);
    }
}
