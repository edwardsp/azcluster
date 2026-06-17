pub mod azcp;
pub mod blobcache;
pub mod deploy;
pub mod feature;
pub mod operators;
pub mod status;
pub mod train;
pub mod validate;

use anyhow::{bail, Context, Result};

pub(crate) fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub(crate) fn output_string(outputs: &serde_json::Value, key: &str) -> Result<Option<String>> {
    let Some(v) = outputs.get(key).and_then(|v| v.get("value")) else {
        return Ok(None);
    };
    if let Some(s) = v.as_str() {
        return Ok(Some(s.to_string()));
    }
    if v.is_null() {
        return Ok(None);
    }
    bail!("deployment output '{key}' was not a string")
}

pub(crate) fn output_u32(outputs: &serde_json::Value, key: &str) -> Result<u32> {
    let v = outputs
        .get(key)
        .and_then(|v| v.get("value"))
        .ok_or_else(|| anyhow::anyhow!("deployment did not return {key}"))?;
    let n = v
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("deployment output '{key}' was not an unsigned integer"))?;
    u32::try_from(n).with_context(|| format!("deployment output '{key}' exceeds u32"))
}
