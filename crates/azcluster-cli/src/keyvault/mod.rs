#![allow(dead_code, unused_imports)]

pub mod client;

pub use client::KeyVaultClient;

pub fn vault_uri(vault_name: &str) -> String {
    format!("https://{vault_name}.vault.azure.net")
}
