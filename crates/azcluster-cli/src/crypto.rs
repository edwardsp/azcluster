use anyhow::{Context, Result};
use rand::rngs::OsRng;
use ssh_key::{private::Ed25519Keypair, LineEnding, PrivateKey};

pub struct AdminKeypair {
    pub public_openssh: String,
    pub private_openssh_pem: String,
}

pub fn generate_admin_keypair(comment: &str) -> Result<AdminKeypair> {
    let kp = Ed25519Keypair::random(&mut OsRng);
    let mut sk = PrivateKey::from(kp);
    sk.set_comment(comment);
    let pub_line = sk
        .public_key()
        .to_openssh()
        .context("serialize ed25519 public key (openssh)")?;
    let priv_pem = sk
        .to_openssh(LineEnding::LF)
        .context("serialize ed25519 private key (openssh)")?
        .to_string();
    Ok(AdminKeypair {
        public_openssh: pub_line,
        private_openssh_pem: priv_pem,
    })
}

pub fn derive_kv_name(subscription_id: &str, cluster_name: &str, location: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(subscription_id.as_bytes());
    hasher.update(b"|");
    hasher.update(cluster_name.as_bytes());
    hasher.update(b"|");
    hasher.update(location.as_bytes());
    let hash = hasher.finalize();
    let hex = hash.to_hex();
    format!("kv-azc-{}", &hex[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_roundtrip_openssh_format() {
        let kp = generate_admin_keypair("test").unwrap();
        assert!(
            kp.public_openssh.starts_with("ssh-ed25519 "),
            "{}",
            kp.public_openssh
        );
        assert!(kp.public_openssh.ends_with(" test"));
        assert!(kp
            .private_openssh_pem
            .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(kp
            .private_openssh_pem
            .trim_end()
            .ends_with("-----END OPENSSH PRIVATE KEY-----"));
    }

    #[test]
    fn kv_name_is_deterministic_and_24_chars_max() {
        let a = derive_kv_name("sub-1", "demo", "eastus");
        let b = derive_kv_name("sub-1", "demo", "eastus");
        assert_eq!(a, b);
        assert!(a.len() <= 24);
        assert!(a.starts_with("kv-azc-"));
        assert_ne!(a, derive_kv_name("sub-2", "demo", "eastus"));
        assert_ne!(a, derive_kv_name("sub-1", "demo2", "eastus"));
        assert_ne!(a, derive_kv_name("sub-1", "demo", "westus"));
    }
}
