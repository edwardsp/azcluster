use anyhow::{bail, Context, Result};
use base64::Engine;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::bastion::client::{BastionClient, BastionTokenResponse};

const WS_OPCODE_CONTINUATION: u8 = 0x00;
const WS_OPCODE_TEXT: u8 = 0x01;
const WS_OPCODE_BINARY: u8 = 0x02;
const WS_OPCODE_CLOSE: u8 = 0x08;
const WS_OPCODE_PING: u8 = 0x09;
const WS_OPCODE_PONG: u8 = 0x0A;

const MAX_FRAME_BYTES: u64 = 16 * 1024 * 1024;

pub async fn run_stdio_bridge(
    client: Arc<BastionClient>,
    bastion_dns: String,
    target_resource_id: String,
    target_port: u16,
) -> Result<()> {
    let token = client
        .get_tunnel_token(&bastion_dns, &target_resource_id, target_port, None, None)
        .await
        .context("fetch Bastion tunnel token")?;
    let tls = ws_connect(&bastion_dns, &token).await?;
    let (ws_read, ws_write) = tokio::io::split(tls);
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let res = bridge(stdin, stdout, ws_read, ws_write).await;

    let _ = client
        .delete_tunnel_token(&bastion_dns, &token.auth_token, token.node_id.as_deref())
        .await;
    res
}

#[allow(dead_code)]
pub async fn run_tcp_listener(
    client: Arc<BastionClient>,
    bastion_dns: String,
    target_resource_id: String,
    target_port: u16,
    local_bind: std::net::SocketAddr,
) -> Result<u16> {
    let listener = TcpListener::bind(local_bind)
        .await
        .with_context(|| format!("bind {local_bind}"))?;
    let actual = listener.local_addr()?.port();
    eprintln!("Listening on 127.0.0.1:{actual} -> bastion -> {target_resource_id}:{target_port}");
    loop {
        let (tcp, peer) = listener
            .accept()
            .await
            .context("accept local tunnel client")?;
        eprintln!("accepted tunnel client {peer}");
        let client = client.clone();
        let bastion_dns = bastion_dns.clone();
        let resource_id = target_resource_id.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_one(client, bastion_dns, resource_id, target_port, tcp).await {
                eprintln!("tunnel client {peer} failed: {e:#}");
            }
        });
    }
}

#[allow(dead_code)]
async fn handle_one(
    client: Arc<BastionClient>,
    bastion_dns: String,
    resource_id: String,
    target_port: u16,
    tcp: TcpStream,
) -> Result<()> {
    let token = client
        .get_tunnel_token(&bastion_dns, &resource_id, target_port, None, None)
        .await?;
    let tls = ws_connect(&bastion_dns, &token).await?;
    let (ws_read, ws_write) = tokio::io::split(tls);
    let (tcp_read, tcp_write) = tcp.into_split();
    let res = bridge(tcp_read, tcp_write, ws_read, ws_write).await;
    let _ = client
        .delete_tunnel_token(&bastion_dns, &token.auth_token, token.node_id.as_deref())
        .await;
    res
}

async fn bridge<R, W, WR, WW>(
    mut local_read: R,
    mut local_write: W,
    ws_read: WR,
    ws_write: WW,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
    WR: AsyncRead + Unpin + Send + 'static,
    WW: AsyncWrite + Unpin + Send + 'static,
{
    let local_to_ws = async move {
        let mut writer = WsWriter { inner: ws_write };
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = local_read.read(&mut buf).await?;
            if n == 0 {
                let _ = writer.write_close().await;
                break;
            }
            writer.write_binary(&buf[..n]).await?;
        }
        Ok::<(), anyhow::Error>(())
    };
    let ws_to_local = async move {
        let mut reader = WsReader { inner: ws_read };
        loop {
            match reader.read_frame().await? {
                WsFrame::Binary(data) | WsFrame::Text(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    local_write.write_all(&data).await?;
                    local_write.flush().await?;
                }
                WsFrame::Ping | WsFrame::Pong => {}
                WsFrame::Close => break,
                WsFrame::Other => {}
            }
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        r = local_to_ws => r,
        r = ws_to_local => r,
    }
}

async fn ws_connect(
    bastion_dns: &str,
    token: &BastionTokenResponse,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let path = match token.node_id.as_deref() {
        Some(nid) => format!("/webtunnelv2/{}?X-Node-Id={}", token.auth_token, nid),
        None => format!("/webtunnelv2/{}", token.auth_token),
    };
    let tcp = TcpStream::connect((bastion_dns, 443))
        .await
        .with_context(|| format!("tcp connect {bastion_dns}:443"))?;
    tcp.set_nodelay(true)?;

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));
    let server_name = rustls::pki_types::ServerName::try_from(bastion_dns.to_string())
        .context("invalid TLS server name")?;
    let mut tls = connector.connect(server_name, tcp).await?;

    let key = base64::engine::general_purpose::STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {bastion_dns}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nOrigin: https://{bastion_dns}\r\n\r\n"
    );
    tls.write_all(req.as_bytes()).await?;
    tls.flush().await?;

    let mut resp = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        tls.read_exact(&mut byte).await?;
        resp.push(byte[0]);
        let n = resp.len();
        if n >= 4 && &resp[n - 4..] == b"\r\n\r\n" {
            break;
        }
        if n > 8192 {
            bail!("WS upgrade response too large");
        }
    }
    let text = String::from_utf8_lossy(&resp);
    let first = text.lines().next().unwrap_or("");
    if !first.contains("101") {
        bail!("Bastion WS upgrade rejected: {}", text.trim());
    }
    Ok(tls)
}

enum WsFrame {
    Binary(Vec<u8>),
    Text(Vec<u8>),
    Ping,
    Pong,
    Close,
    Other,
}

struct WsReader<R> {
    inner: R,
}

impl<R: AsyncRead + Unpin> WsReader<R> {
    async fn read_frame(&mut self) -> Result<WsFrame> {
        let mut h = [0u8; 2];
        self.inner.read_exact(&mut h).await?;
        let opcode = h[0] & 0x0F;
        let masked = h[1] & 0x80 != 0;
        let len_byte = h[1] & 0x7F;
        let payload_len: u64 = match len_byte {
            126 => {
                let mut b = [0u8; 2];
                self.inner.read_exact(&mut b).await?;
                u16::from_be_bytes(b) as u64
            }
            127 => {
                let mut b = [0u8; 8];
                self.inner.read_exact(&mut b).await?;
                u64::from_be_bytes(b)
            }
            n => n as u64,
        };
        if payload_len > MAX_FRAME_BYTES {
            bail!("ws frame too large: {payload_len}");
        }
        let mask_key = if masked {
            let mut k = [0u8; 4];
            self.inner.read_exact(&mut k).await?;
            Some(k)
        } else {
            None
        };
        let mut payload = vec![0u8; payload_len as usize];
        if !payload.is_empty() {
            self.inner.read_exact(&mut payload).await?;
        }
        if let Some(key) = mask_key {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= key[i % 4];
            }
        }
        Ok(match opcode {
            WS_OPCODE_BINARY | WS_OPCODE_CONTINUATION => WsFrame::Binary(payload),
            WS_OPCODE_TEXT => WsFrame::Text(payload),
            WS_OPCODE_PING => WsFrame::Ping,
            WS_OPCODE_PONG => WsFrame::Pong,
            WS_OPCODE_CLOSE => WsFrame::Close,
            _ => WsFrame::Other,
        })
    }
}

struct WsWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> WsWriter<W> {
    async fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<()> {
        let mask_key = mask_key_random();
        let fin_opcode = 0x80 | opcode;
        let len = payload.len();
        if len < 126 {
            self.inner
                .write_all(&[fin_opcode, 0x80 | len as u8])
                .await?;
        } else if len <= u16::MAX as usize {
            self.inner.write_all(&[fin_opcode, 0x80 | 126]).await?;
            self.inner.write_all(&(len as u16).to_be_bytes()).await?;
        } else {
            self.inner.write_all(&[fin_opcode, 0x80 | 127]).await?;
            self.inner.write_all(&(len as u64).to_be_bytes()).await?;
        }
        self.inner.write_all(&mask_key).await?;
        let mut masked = payload.to_vec();
        for (i, b) in masked.iter_mut().enumerate() {
            *b ^= mask_key[i % 4];
        }
        self.inner.write_all(&masked).await?;
        self.inner.flush().await?;
        Ok(())
    }

    async fn write_binary(&mut self, data: &[u8]) -> Result<()> {
        self.write_frame(WS_OPCODE_BINARY, data).await
    }

    async fn write_close(&mut self) -> Result<()> {
        self.write_frame(WS_OPCODE_CLOSE, &[]).await
    }
}

fn mask_key_random() -> [u8; 4] {
    let bytes = uuid::Uuid::new_v4();
    let b = bytes.as_bytes();
    [b[0], b[1], b[2], b[3]]
}
