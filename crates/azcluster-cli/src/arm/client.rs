//! Azure Resource Manager (ARM) REST client.
//!
//! Provides a generic, blocking HTTP client for ARM operations.
//! Replaces shell-out calls to `az` CLI with direct REST API calls.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;
/// API versions for different Azure resource providers.
pub struct ApiVersions {
    pub resource_group: String,
    pub deployment: String,
    pub compute: String,
    pub network: String,
    pub storage: String,
    pub grafana: String,
}

impl Default for ApiVersions {
    fn default() -> Self {
        Self {
            resource_group: "2024-03-01".to_string(),
            deployment: "2024-03-01".to_string(),
            compute: "2024-07-01".to_string(),
            network: "2023-11-01".to_string(),
            storage: "2023-05-01".to_string(),
            grafana: "2023-09-01".to_string(),
        }
    }
}

/// Azure Resource Manager REST client.
pub struct ArmClient {
    client: reqwest::blocking::Client,
    subscription_id: String,
    access_token: String,
    api_versions: ApiVersions,
}

impl ArmClient {
    /// Create a new ARM client.
    pub fn new(access_token: String, subscription_id: String) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            subscription_id,
            access_token,
            api_versions: ApiVersions::default(),
        })
    }

    /// Get the subscription ID.
    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }

    /// Set custom API versions.
    pub fn with_api_versions(mut self, versions: ApiVersions) -> Self {
        self.api_versions = versions;
        self
    }

    /// Make a GET request to the ARM API.
    fn get(&self, url: &str) -> Result<Value> {
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.access_token)
            .send()
            .context("Failed to send GET request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            bail!("ARM GET failed ({status}): {body}");
        }

        response.json().context("Failed to parse ARM response")
    }

    /// Make a PUT request to the ARM API.
    fn put(&self, url: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .put(url)
            .bearer_auth(&self.access_token)
            .json(&body)
            .send()
            .context("Failed to send PUT request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            bail!("ARM PUT failed ({status}): {body}");
        }

        response.json().context("Failed to parse ARM response")
    }

    /// Make a POST request to the ARM API.
    fn post(&self, url: &str, body: Option<Value>) -> Result<Value> {
        let mut req = self.client.post(url).bearer_auth(&self.access_token);

        if let Some(b) = body {
            req = req.json(&b);
        }

        let response = req.send().context("Failed to send POST request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            bail!("ARM POST failed ({status}): {body}");
        }

        response.json().context("Failed to parse ARM response")
    }

    /// Make a DELETE request to the ARM API.
    fn delete(&self, url: &str) -> Result<()> {
        let response = self
            .client
            .delete(url)
            .bearer_auth(&self.access_token)
            .send()
            .context("Failed to send DELETE request")?;

        if !response.status().is_success() && response.status() != 204 {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            bail!("ARM DELETE failed ({status}): {body}");
        }

        Ok(())
    }

    fn patch_and_wait(&self, url: &str, body: Value) -> Result<()> {
        let response = self
            .client
            .patch(url)
            .bearer_auth(&self.access_token)
            .json(&body)
            .send()
            .context("Failed to send PATCH request")?;

        let status = response.status();
        if !status.is_success() {
            let err_body = response.text().unwrap_or_default();
            bail!("ARM PATCH failed ({status}): {err_body}");
        }

        let async_url = response
            .headers()
            .get("azure-asyncoperation")
            .or_else(|| response.headers().get("location"))
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        if status.as_u16() == 200 {
            return Ok(());
        }

        let Some(async_url) = async_url else {
            return Ok(());
        };

        self.wait_for_async_operation(&async_url)
    }

    fn wait_for_async_operation(&self, async_url: &str) -> Result<()> {
        use std::time::{Duration, Instant};
        const MAX_WAIT_SECS: u64 = 30 * 60;
        const POLL_INTERVAL_SECS: u64 = 10;
        let started = Instant::now();
        loop {
            let response = self
                .client
                .get(async_url)
                .bearer_auth(&self.access_token)
                .send()
                .context("Failed to poll async operation")?;
            let http_status = response.status();
            if !http_status.is_success() {
                let body = response.text().unwrap_or_default();
                bail!("async-operation poll failed ({http_status}): {body}");
            }
            let v: Value = response
                .json()
                .context("Failed to parse async-operation response")?;
            let op_status = v
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            match op_status.as_str() {
                "Succeeded" => return Ok(()),
                "Failed" | "Canceled" => {
                    let err = v
                        .get("error")
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| op_status.clone());
                    bail!("async-operation terminated with status {op_status}: {err}");
                }
                _ => {}
            }
            if started.elapsed().as_secs() > MAX_WAIT_SECS {
                bail!("async-operation still '{op_status}' after {MAX_WAIT_SECS}s");
            }
            std::thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        }
    }

    /// List resources with pagination support.
    fn list_paginated(&self, url: &str) -> Result<Vec<Value>> {
        let mut results = Vec::new();
        let mut next_url = Some(url.to_string());

        while let Some(url) = next_url {
            let response = self.get(&url)?;

            if let Some(values) = response.get("value").and_then(|v| v.as_array()) {
                results.extend(values.clone());
            }

            next_url = response
                .get("nextLink")
                .and_then(|v| v.as_str())
                .map(String::from);
        }

        Ok(results)
    }

    /// Get a resource group.
    pub fn get_resource_group(&self, name: &str) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourcegroups/{}?api-version={}",
            self.subscription_id, name, self.api_versions.resource_group
        );
        self.get(&url)
    }

    /// PATCH the tags of a resource group (merge semantics on the server side).
    pub fn patch_resource_group_tags(
        &self,
        name: &str,
        tags: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourcegroups/{}?api-version={}",
            self.subscription_id, name, self.api_versions.resource_group
        );
        self.patch_and_wait(&url, json!({ "tags": tags }))
    }

    /// Create or update a resource group.
    pub fn create_resource_group(
        &self,
        name: &str,
        location: &str,
        tags: Option<Value>,
    ) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourcegroups/{}?api-version={}",
            self.subscription_id, name, self.api_versions.resource_group
        );

        let mut body = json!({
            "location": location,
        });

        if let Some(t) = tags {
            body["tags"] = t;
        }

        self.put(&url, body)
    }

    /// Delete a resource group (async operation).
    pub fn delete_resource_group(&self, name: &str) -> Result<()> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourcegroups/{}?api-version={}",
            self.subscription_id, name, self.api_versions.resource_group
        );
        self.delete(&url)
    }

    pub fn list_resource_groups(&self) -> Result<Vec<Value>> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourcegroups?api-version={}",
            self.subscription_id, self.api_versions.resource_group
        );
        self.list_paginated(&url)
    }

    pub fn list_resource_groups_by_tag(
        &self,
        tag_name: &str,
        tag_value: Option<&str>,
    ) -> Result<Vec<Value>> {
        let all = self.list_resource_groups()?;
        Ok(filter_rgs_by_tag(all, tag_name, tag_value))
    }

    pub fn get_resource_group_tags(
        &self,
        name: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let rg = self.get_resource_group(name)?;
        let mut out = std::collections::HashMap::new();
        if let Some(tags) = rg.get("tags").and_then(|t| t.as_object()) {
            for (k, v) in tags {
                if let Some(s) = v.as_str() {
                    out.insert(k.clone(), s.to_string());
                }
            }
        }
        Ok(out)
    }

    // ARM REST subscription-level body shape: `location` at root,
    // `properties.{template,parameters,mode}`, parameter values MUST be
    // wrapped `{"value": <v>}` (or `{"reference": {...}}`). Wrong shape =
    // silent param injection or 400 from ARM.
    pub fn create_subscription_deployment(
        &self,
        deployment_name: &str,
        location: &str,
        template: Value,
        parameters: Value,
    ) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Resources/deployments/{}?api-version={}",
            self.subscription_id, deployment_name, self.api_versions.deployment
        );
        let body = json!({
            "location": location,
            "properties": {
                "template": template,
                "parameters": parameters,
                "mode": "Incremental",
            }
        });
        self.put(&url, body)
    }

    pub fn whatif_subscription_deployment(
        &self,
        deployment_name: &str,
        location: &str,
        template: Value,
        parameters: Value,
    ) -> Result<Value> {
        use std::time::{Duration, Instant};
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Resources/deployments/{}/whatIf?api-version={}",
            self.subscription_id, deployment_name, self.api_versions.deployment
        );
        let body = json!({
            "location": location,
            "properties": {
                "template": template,
                "parameters": parameters,
                "mode": "Incremental",
            }
        });
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&body)
            .send()
            .context("Failed to send whatIf POST")?;
        let status = response.status();
        if status.as_u16() == 200 {
            return response.json().context("parse whatIf result");
        }
        if status.as_u16() != 202 {
            let err = response.text().unwrap_or_default();
            bail!("whatIf POST failed ({status}): {err}");
        }
        let location_url = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .ok_or_else(|| anyhow!("whatIf 202 missing Location header"))?;
        const MAX_WAIT_SECS: u64 = 30 * 60;
        const POLL_INTERVAL_SECS: u64 = 10;
        let started = Instant::now();
        loop {
            let r = self
                .client
                .get(&location_url)
                .bearer_auth(&self.access_token)
                .send()
                .context("poll whatIf")?;
            let st = r.status();
            if st.as_u16() == 202 {
                if started.elapsed().as_secs() > MAX_WAIT_SECS {
                    bail!("whatIf polling exceeded 30 min");
                }
                std::thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
                continue;
            }
            if !st.is_success() {
                let body = r.text().unwrap_or_default();
                bail!("whatIf poll failed ({st}): {body}");
            }
            return r.json().context("parse whatIf final result");
        }
    }

    pub fn wait_for_deployment_completion(&self, deployment_name: &str) -> Result<Value> {
        self.wait_for_deployment_completion_with_progress(deployment_name, &mut |_| {})
    }

    pub fn wait_for_deployment_completion_with_progress(
        &self,
        deployment_name: &str,
        on_tick: &mut dyn FnMut(&[Value]),
    ) -> Result<Value> {
        use std::time::{Duration, Instant};
        const MAX_WAIT_SECS: u64 = 90 * 60;
        const STATE_POLL_SECS: u64 = 15;
        const OPS_POLL_SECS: u64 = 5;
        let started = Instant::now();
        let mut last_state_poll = Instant::now() - Duration::from_secs(STATE_POLL_SECS);
        loop {
            let elapsed_since_state = last_state_poll.elapsed().as_secs();
            if elapsed_since_state >= STATE_POLL_SECS {
                let v = self.get_deployment(deployment_name)?;
                last_state_poll = Instant::now();
                let state = v
                    .get("properties")
                    .and_then(|p| p.get("provisioningState"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if matches!(state, "Succeeded" | "Failed" | "Canceled") {
                    if let Ok(ops) = self.list_subscription_deployment_operations(deployment_name) {
                        on_tick(&ops);
                    }
                    return Ok(v);
                }
                if started.elapsed().as_secs() > MAX_WAIT_SECS {
                    bail!(
                        "deployment '{}' did not reach terminal state within 90 min (last: {state})",
                        deployment_name
                    );
                }
            }
            if let Ok(ops) = self.list_subscription_deployment_operations(deployment_name) {
                on_tick(&ops);
            }
            std::thread::sleep(Duration::from_secs(OPS_POLL_SECS));
        }
    }

    /// Get deployment status.
    pub fn get_deployment(&self, deployment_name: &str) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Resources/deployments/{}?api-version={}",
            self.subscription_id, deployment_name, self.api_versions.deployment
        );
        self.get(&url)
    }

    /// List deployments in a resource group.
    pub fn list_deployments(&self, resource_group: &str) -> Result<Vec<Value>> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Resources/deployments?api-version={}",
            self.subscription_id, resource_group, self.api_versions.deployment
        );
        self.list_paginated(&url)
    }

    pub fn list_subscription_deployment_operations(
        &self,
        deployment_name: &str,
    ) -> Result<Vec<Value>> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Resources/deployments/{}/operations?api-version={}",
            self.subscription_id, deployment_name, self.api_versions.deployment
        );
        self.list_paginated(&url)
    }

    pub fn list_resource_group_deployment_operations(
        &self,
        resource_group: &str,
        deployment_name: &str,
    ) -> Result<Vec<Value>> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Resources/deployments/{}/operations?api-version={}",
            self.subscription_id, resource_group, deployment_name, self.api_versions.deployment
        );
        self.list_paginated(&url)
    }

    /// Get deployment operations with timing information.
    pub fn get_deployment_operations_with_timings(
        &self,
        deployment_name: &str,
    ) -> Result<Vec<Value>> {
        let operations = self.list_subscription_deployment_operations(deployment_name)?;

        // Filter and extract timing information from operations.
        let mut results = Vec::new();
        for op in operations {
            if let Some(props) = op.get("properties") {
                // Extract resource type, name, provisioning state, and duration.
                let resource_type = op
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|id| id.split('/').find(|s| s.contains("providers")))
                    .unwrap_or("Unknown")
                    .to_string();

                let provisioning_state = props
                    .get("provisioningState")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();

                let duration = props
                    .get("duration")
                    .and_then(|v| v.as_str())
                    .and_then(parse_iso8601_duration)
                    .unwrap_or(0.0);

                results.push(json!({
                    "resourceType": resource_type,
                    "provisioningState": provisioning_state,
                    "duration_seconds": duration,
                }));
            }
        }

        Ok(results)
    }

    pub fn get_vmss(&self, resource_group: &str, name: &str) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/virtualMachineScaleSets/{}?api-version={}",
            self.subscription_id, resource_group, name, self.api_versions.compute
        );
        self.get(&url)
    }

    pub fn scale_vmss(&self, resource_group: &str, name: &str, new_capacity: u32) -> Result<()> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/virtualMachineScaleSets/{}?api-version={}",
            self.subscription_id, resource_group, name, self.api_versions.compute
        );
        // PATCH only `sku.capacity`. Re-emitting `identity` triggers AzSecPack
        // LinkedAuthorizationFailed (see AGENTS.md vmss gotcha).
        let body = json!({ "sku": { "capacity": new_capacity } });
        self.patch_and_wait(&url, body)
    }

    pub fn get_grafana_endpoint(&self, resource_group: &str, name: &str) -> Result<String> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Dashboard/grafana/{}?api-version={}",
            self.subscription_id, resource_group, name, self.api_versions.grafana
        );
        let v = self.get(&url)?;
        v.get("properties")
            .and_then(|p| p.get("endpoint"))
            .and_then(|e| e.as_str())
            .map(String::from)
            .ok_or_else(|| anyhow!("AMG {name} has no properties.endpoint"))
    }

    /// List soft-deleted Key Vaults in the current subscription.
    /// Returns the `value` array of `Microsoft.KeyVault/deletedVaults` resources;
    /// each entry has `name`, `properties.location`, `properties.deletionDate`,
    /// `properties.scheduledPurgeDate`, `properties.vaultId`.
    pub fn list_deleted_vaults(&self) -> Result<Vec<Value>> {
        const KV_API: &str = "2023-07-01";
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.KeyVault/deletedVaults?api-version={}",
            self.subscription_id, KV_API
        );
        self.list_paginated(&url)
    }

    /// Purge a soft-deleted Key Vault (permanent — bypasses the 7-day retention).
    /// Returns once the operation has reached a terminal state (handles both
    /// 200-sync and 202-async response shapes).
    pub fn purge_deleted_vault(&self, location: &str, name: &str) -> Result<()> {
        const KV_API: &str = "2023-07-01";
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.KeyVault/locations/{}/deletedVaults/{}/purge?api-version={}",
            self.subscription_id, location, name, KV_API
        );
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.access_token)
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .send()
            .context("Failed to send Key Vault purge POST")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            bail!("Key Vault purge failed ({status}): {body}");
        }
        if status.as_u16() == 200 || status.as_u16() == 204 {
            return Ok(());
        }
        let async_url = response
            .headers()
            .get("azure-asyncoperation")
            .or_else(|| response.headers().get("location"))
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        match async_url {
            Some(u) => self.wait_for_async_operation(&u),
            None => Ok(()),
        }
    }
}

/// Parse ISO8601 duration string (e.g., "PT1H30M45S") to seconds.
fn parse_iso8601_duration(s: &str) -> Option<f64> {
    let s = s.strip_prefix("PT")?;
    let mut total: f64 = 0.0;
    let mut buf = String::new();
    for ch in s.chars() {
        match ch {
            '0'..='9' | '.' => buf.push(ch),
            'H' => {
                total += buf.parse::<f64>().ok()? * 3600.0;
                buf.clear();
            }
            'M' => {
                total += buf.parse::<f64>().ok()? * 60.0;
                buf.clear();
            }
            'S' => {
                total += buf.parse::<f64>().ok()?;
                buf.clear();
            }
            _ => return None,
        }
    }
    Some(total)
}

fn filter_rgs_by_tag(rgs: Vec<Value>, tag_name: &str, tag_value: Option<&str>) -> Vec<Value> {
    rgs.into_iter()
        .filter(|rg| match rg.get("tags").and_then(|t| t.get(tag_name)) {
            Some(v) => match (v.as_str(), tag_value) {
                (Some(s), Some(want)) => s == want,
                (Some(_), None) => true,
                _ => false,
            },
            None => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_versions_default() {
        let versions = ApiVersions::default();
        assert_eq!(versions.resource_group, "2024-03-01");
        assert_eq!(versions.deployment, "2024-03-01");
        assert_eq!(versions.compute, "2024-07-01");
    }

    #[test]
    fn test_arm_client_new() {
        let client = ArmClient::new("token".to_string(), "sub-123".to_string()).unwrap();
        assert_eq!(client.subscription_id(), "sub-123");
    }

    #[test]
    fn test_parse_iso8601_duration() {
        assert_eq!(parse_iso8601_duration("PT1H30M45S"), Some(5445.0));
        assert_eq!(parse_iso8601_duration("PT30S"), Some(30.0));
        assert_eq!(parse_iso8601_duration("PT1M"), Some(60.0));
        assert_eq!(parse_iso8601_duration("PT1H"), Some(3600.0));
    }

    fn rg(name: &str, tags: Value) -> Value {
        json!({"name": name, "tags": tags})
    }

    #[test]
    fn filter_rgs_by_tag_matches_name_and_value() {
        let rgs = vec![
            rg(
                "a",
                json!({"azcluster:managed": "true", "azcluster:name": "alpha"}),
            ),
            rg(
                "b",
                json!({"azcluster:managed": "true", "azcluster:name": "beta"}),
            ),
            rg("c", json!({"other": "x"})),
            rg("d", json!({})),
        ];
        let by_managed = filter_rgs_by_tag(rgs.clone(), "azcluster:managed", Some("true"));
        assert_eq!(by_managed.len(), 2);

        let by_name = filter_rgs_by_tag(rgs.clone(), "azcluster:name", Some("beta"));
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].get("name").and_then(|v| v.as_str()), Some("b"));

        let any_managed = filter_rgs_by_tag(rgs, "azcluster:managed", None);
        assert_eq!(any_managed.len(), 2);
    }

    #[test]
    fn filter_rgs_by_tag_missing_tag_excluded() {
        let rgs = vec![rg("a", json!({}))];
        assert!(filter_rgs_by_tag(rgs, "azcluster:managed", Some("true")).is_empty());
    }

    #[test]
    fn filter_rgs_by_tag_value_mismatch_excluded() {
        let rgs = vec![rg("a", json!({"azcluster:managed": "false"}))];
        assert!(filter_rgs_by_tag(rgs, "azcluster:managed", Some("true")).is_empty());
    }
}
