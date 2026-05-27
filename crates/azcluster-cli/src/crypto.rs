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

/// Derive a deterministic storage account name from the same triple used for the
/// Key Vault. Azure Storage account names are 3-24 chars, lowercase alphanumeric
/// ONLY (no hyphens, no underscores) and globally unique. The `stazc` prefix
/// + 8 hex chars from blake3 fits in 13 chars.
///
/// Collision probability across a single subscription's accounts is negligible.
pub fn derive_storage_account_name(
    subscription_id: &str,
    cluster_name: &str,
    location: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(subscription_id.as_bytes());
    hasher.update(b"|");
    hasher.update(cluster_name.as_bytes());
    hasher.update(b"|");
    hasher.update(location.as_bytes());
    let hash = hasher.finalize();
    let hex = hash.to_hex();
    format!("stazc{}", &hex[..8])
}

/// Validate an operator-supplied --storage-name override. Azure Storage account
/// names must be 3-24 chars, lowercase ASCII letters + digits ONLY.
pub fn validate_storage_account_name(name: &str) -> anyhow::Result<()> {
    if !(3..=24).contains(&name.len()) {
        anyhow::bail!(
            "storage account name '{name}' must be 3-24 characters (got {})",
            name.len()
        );
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        anyhow::bail!(
            "storage account name '{name}' must contain only lowercase ASCII letters and digits"
        );
    }
    Ok(())
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

    #[test]
    fn storage_account_name_is_deterministic_lowercase_alnum_under_24() {
        let a = derive_storage_account_name("sub-1", "demo", "eastus");
        let b = derive_storage_account_name("sub-1", "demo", "eastus");
        assert_eq!(a, b);
        assert!(a.len() <= 24);
        assert!(a.len() >= 3);
        assert!(a.starts_with("stazc"));
        assert!(a
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_ne!(a, derive_storage_account_name("sub-2", "demo", "eastus"));
        assert_ne!(a, derive_storage_account_name("sub-1", "demo2", "eastus"));
        assert_ne!(a, derive_storage_account_name("sub-1", "demo", "westus"));
    }

    #[test]
    fn validate_storage_account_name_accepts_valid_names() {
        assert!(validate_storage_account_name("stazc89a3f12c").is_ok());
        assert!(validate_storage_account_name("myacc123").is_ok());
        assert!(validate_storage_account_name("abc").is_ok());
        assert!(validate_storage_account_name(&"a".repeat(24)).is_ok());
    }

    #[test]
    fn validate_storage_account_name_rejects_invalid_names() {
        assert!(validate_storage_account_name("ab").is_err());
        assert!(validate_storage_account_name(&"a".repeat(25)).is_err());
        assert!(validate_storage_account_name("My-Account").is_err());
        assert!(validate_storage_account_name("my_account").is_err());
        assert!(validate_storage_account_name("MyAccount").is_err());
        assert!(validate_storage_account_name("acc!").is_err());
    }
}
