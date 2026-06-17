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

/// A single deployment operation tagged with its parent module deployment
/// name and tree depth, produced by [`ArmClient::list_deployment_operations_recursive`].
///
/// `parent` is the deployment name the op was fetched from (the immediate
/// parent in the deployment tree); `depth` is 0 for the root sub-scope
/// deployment, 1 for its direct nested modules, and so on. `op` is the raw
/// ARM `operations` envelope, preserved so callers can read any field
/// without API churn.
#[derive(Debug, Clone)]
pub struct DeploymentOp {
    pub parent: String,
    pub depth: u8,
    pub op: Value,
}

/// Result of an AKS `runCommand` invocation. `exit_code` is the shell exit code
/// of the command that ran inside the in-cluster pod (0 = success); a Succeeded
/// `provisioning_state` only means the command FINISHED, not that it succeeded —
/// callers MUST check `exit_code`.
#[derive(Debug, Clone)]
pub struct RunCommandResult {
    pub exit_code: i64,
    pub logs: String,
    pub provisioning_state: String,
}

fn parse_run_command_result(v: &Value) -> Option<RunCommandResult> {
    let props = v.get("properties")?;
    let provisioning_state = props
        .get("provisioningState")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    if provisioning_state.is_empty() {
        return None;
    }
    Some(RunCommandResult {
        exit_code: props.get("exitCode").and_then(|c| c.as_i64()).unwrap_or(-1),
        logs: props
            .get("logs")
            .and_then(|l| l.as_str())
            .unwrap_or("")
            .to_string(),
        provisioning_state,
    })
}

/// Inspect a deployment-operation envelope. If its target is itself a
/// nested `Microsoft.Resources/deployments`, return `(resource_group, name)`.
/// `resource_group` is empty for sub-scope nested deployments.
fn nested_module_target(op: &Value) -> Option<(String, String)> {
    let target = op.pointer("/properties/targetResource")?;
    let rtype = target
        .get("resourceType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !rtype.eq_ignore_ascii_case("Microsoft.Resources/deployments") {
        return None;
    }
    let name = target
        .get("resourceName")
        .and_then(|v| v.as_str())
        .map(String::from)?;
    if name.is_empty() {
        return None;
    }
    let rg = target
        .get("resourceGroup")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            target
                .get("id")
                .and_then(|v| v.as_str())
                .and_then(rg_from_resource_id)
        })
        .unwrap_or_default();
    Some((rg, name))
}

fn rg_from_resource_id(id: &str) -> Option<String> {
    let mut parts = id.split('/').skip(1);
    while let Some(seg) = parts.next() {
        if seg.eq_ignore_ascii_case("resourceGroups") {
            return parts.next().map(String::from);
        }
    }
    None
}

fn dedup_ops_by_target(ops: Vec<Value>) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for op in ops {
        let id = op
            .pointer("/properties/targetResource/id")
            .and_then(|v| v.as_str())
            .map(String::from);
        if let Some(id) = id {
            if !latest.contains_key(&id) {
                order.push(id.clone());
            }
            latest.insert(id, op);
        }
    }
    order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect()
}

/// Azure Resource Manager REST client.
pub struct ArmClient {
    client: reqwest::blocking::Client,
    subscription_id: String,
    access_token: std::sync::RwLock<String>,
    api_versions: ApiVersions,
    refresh_token_fn: Option<Box<dyn Fn() -> Result<String> + Send + Sync>>,
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
            access_token: std::sync::RwLock::new(access_token),
            api_versions: ApiVersions::default(),
            refresh_token_fn: None,
        })
    }

    /// Install a token-refresh callback used transparently when the cached
    /// token returns `401 ExpiredAuthenticationToken`. Long-running poll loops
    /// (deployment completion, async LROs) need this because the OAuth2
    /// access token typically lives ~75 min while real Azure deployments
    /// can run longer.
    pub fn with_refresh_callback<F>(mut self, f: F) -> Self
    where
        F: Fn() -> Result<String> + Send + Sync + 'static,
    {
        self.refresh_token_fn = Some(Box::new(f));
        self
    }

    fn access_token(&self) -> String {
        self.access_token.read().unwrap().clone()
    }

    fn try_refresh_token(&self) -> Result<bool> {
        let Some(refresh_fn) = self.refresh_token_fn.as_ref() else {
            return Ok(false);
        };
        let new_token = refresh_fn().context("token refresh failed")?;
        *self.access_token.write().unwrap() = new_token;
        Ok(true)
    }

    fn is_expired_token_error(status: reqwest::StatusCode, body: &str) -> bool {
        status.as_u16() == 401
            && (body.contains("ExpiredAuthenticationToken")
                || body.contains("ExpiredToken")
                || body.contains("InvalidAuthenticationToken"))
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

    /// Make a GET request to the ARM API. On 401 ExpiredAuthenticationToken,
    /// transparently refresh the OAuth2 token (if a callback was installed) and
    /// retry once. Long deploy polls (>75 min) outlive the token TTL otherwise.
    fn get(&self, url: &str) -> Result<Value> {
        for attempt in 0..=1 {
            let response = self
                .client
                .get(url)
                .bearer_auth(self.access_token())
                .send()
                .context("Failed to send GET request")?;
            let status = response.status();
            if status.is_success() {
                return response.json().context("Failed to parse ARM response");
            }
            let body = response.text().unwrap_or_default();
            if attempt == 0
                && Self::is_expired_token_error(status, &body)
                && self.try_refresh_token()?
            {
                continue;
            }
            bail!("ARM GET failed ({status}): {body}");
        }
        unreachable!()
    }

    /// Make a PUT request to the ARM API.
    fn put(&self, url: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .put(url)
            .bearer_auth(self.access_token())
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
        let mut req = self.client.post(url).bearer_auth(self.access_token());

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
            .bearer_auth(self.access_token())
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
            .bearer_auth(self.access_token())
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
                .bearer_auth(self.access_token())
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
            .bearer_auth(self.access_token())
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
                .bearer_auth(self.access_token())
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
        on_tick: &mut dyn FnMut(&[DeploymentOp]),
    ) -> Result<Value> {
        use std::time::{Duration, Instant};
        const MAX_WAIT_SECS: u64 = 90 * 60;
        const STATE_POLL_SECS: u64 = 15;
        const OPS_POLL_SECS: u64 = 10;
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
                    let ops = self.list_deployment_operations_recursive(deployment_name);
                    on_tick(&ops);
                    return Ok(v);
                }
                if started.elapsed().as_secs() > MAX_WAIT_SECS {
                    bail!(
                        "deployment '{}' did not reach terminal state within 90 min (last: {state})",
                        deployment_name
                    );
                }
            }
            let ops = self.list_deployment_operations_recursive(deployment_name);
            on_tick(&ops);
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

    pub fn list_deployment_operations_recursive(&self, deployment: &str) -> Vec<DeploymentOp> {
        let mut out = Vec::new();
        self.collect_sub_ops_into(&mut out, deployment, 0);
        out
    }

    fn collect_sub_ops_into(&self, out: &mut Vec<DeploymentOp>, deployment: &str, depth: u8) {
        let ops = match self.list_subscription_deployment_operations(deployment) {
            Ok(v) => v,
            Err(_) => return,
        };
        for op in dedup_ops_by_target(ops) {
            let nested = nested_module_target(&op);
            out.push(DeploymentOp {
                parent: deployment.to_string(),
                depth,
                op,
            });
            if let Some((rg, name)) = nested {
                if rg.is_empty() {
                    self.collect_sub_ops_into(out, &name, depth.saturating_add(1));
                } else {
                    self.collect_group_ops_into(out, &rg, &name, depth.saturating_add(1));
                }
            }
        }
    }

    fn collect_group_ops_into(
        &self,
        out: &mut Vec<DeploymentOp>,
        rg: &str,
        deployment: &str,
        depth: u8,
    ) {
        let ops = match self.list_resource_group_deployment_operations(rg, deployment) {
            Ok(v) => v,
            Err(_) => return,
        };
        for op in dedup_ops_by_target(ops) {
            let nested = nested_module_target(&op);
            out.push(DeploymentOp {
                parent: deployment.to_string(),
                depth,
                op,
            });
            if let Some((_, name)) = nested {
                self.collect_group_ops_into(out, rg, &name, depth.saturating_add(1));
            }
        }
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
            .bearer_auth(self.access_token())
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

    fn post_empty(&self, url: &str) -> Result<Value> {
        for attempt in 0..=1 {
            let response = self
                .client
                .post(url)
                .bearer_auth(self.access_token())
                .header(reqwest::header::CONTENT_LENGTH, "0")
                .send()
                .context("Failed to send POST request")?;
            let status = response.status();
            if status.is_success() {
                return response.json().context("Failed to parse ARM response");
            }
            let body = response.text().unwrap_or_default();
            if attempt == 0
                && Self::is_expired_token_error(status, &body)
                && self.try_refresh_token()?
            {
                continue;
            }
            bail!("ARM POST failed ({status}): {body}");
        }
        unreachable!()
    }

    pub fn register_feature(&self, provider_ns: &str, feature: &str) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Features/providers/{}/features/{}/register?api-version=2021-07-01",
            self.subscription_id, provider_ns, feature
        );
        self.post_empty(&url)
    }

    pub fn get_feature(&self, provider_ns: &str, feature: &str) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Features/providers/{}/features/{}?api-version=2021-07-01",
            self.subscription_id, provider_ns, feature
        );
        self.get(&url)
    }

    pub fn register_provider(&self, provider_ns: &str) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/{}/register?api-version=2022-09-01",
            self.subscription_id, provider_ns
        );
        self.post_empty(&url)
    }

    /// Run a shell command inside an ephemeral pod in an AKS cluster via the
    /// `runCommand` ARM action (kubectl + helm are preinstalled in the pod).
    /// Polls `commandResults/{id}` to terminal state. ARM RBAC `.../runCommand/action`
    /// is required; no laptop kubeconfig/helm/kubectl is involved.
    pub fn aks_run_command(
        &self,
        resource_group: &str,
        cluster: &str,
        command: &str,
        context_b64: Option<&str>,
    ) -> Result<RunCommandResult> {
        const AKS_API: &str = "2024-09-01";
        let run_url = format!(
            "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.ContainerService/managedClusters/{}/runCommand?api-version={}",
            self.subscription_id, resource_group, cluster, AKS_API
        );
        let mut body = json!({ "command": command });
        if let Some(ctx) = context_b64 {
            body["context"] = json!(ctx);
        }
        let response = self
            .client
            .post(&run_url)
            .bearer_auth(self.access_token())
            .json(&body)
            .send()
            .context("Failed to send AKS runCommand POST")?;
        let status = response.status();
        if !status.is_success() {
            let b = response.text().unwrap_or_default();
            bail!("AKS runCommand POST failed ({status}): {b}");
        }
        // runCommand returns 202 with a `Location` header pointing at the
        // commandResults resource; the 202 body is often empty, so headers are
        // read first and the body is parsed leniently (no unconditional json()).
        let header_url = response
            .headers()
            .get("location")
            .or_else(|| response.headers().get("azure-asyncoperation"))
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let text = response.text().unwrap_or_default();
        let body_val: Option<Value> = serde_json::from_str(&text).ok();
        if let Some(v) = body_val.as_ref() {
            if let Some(r) = parse_run_command_result(v) {
                if matches!(r.provisioning_state.as_str(), "Succeeded" | "Failed") {
                    return Ok(r);
                }
            }
        }
        let result_url = match header_url {
            Some(u) => u,
            None => {
                let command_id = body_val
                    .as_ref()
                    .and_then(|v| v.get("id").or_else(|| v.get("name")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
                    .ok_or_else(|| {
                        anyhow!("AKS runCommand: no Location header and no command id in response")
                    })?;
                format!(
                    "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.ContainerService/managedClusters/{}/commandResults/{}?api-version={}",
                    self.subscription_id, resource_group, cluster, command_id, AKS_API
                )
            }
        };
        use std::time::{Duration, Instant};
        const MAX_WAIT_SECS: u64 = 30 * 60;
        const POLL_INTERVAL_SECS: u64 = 10;
        let started = Instant::now();
        loop {
            let r = self
                .client
                .get(&result_url)
                .bearer_auth(self.access_token())
                .send()
                .context("poll AKS runCommand result")?;
            let st = r.status();
            if st.is_success() {
                let t = r.text().unwrap_or_default();
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    if let Some(res) = parse_run_command_result(&v) {
                        if matches!(
                            res.provisioning_state.as_str(),
                            "Succeeded" | "Failed" | "Canceled"
                        ) {
                            return Ok(res);
                        }
                    }
                }
            } else if st.as_u16() == 401 {
                // Best-effort token refresh; the next poll iteration retries.
                self.try_refresh_token().ok();
            }
            if started.elapsed().as_secs() > MAX_WAIT_SECS {
                bail!("AKS runCommand on '{cluster}' still running after {MAX_WAIT_SECS}s");
            }
            std::thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
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
        .filter(|rg| {
            if let Some(name) = rg.get("name").and_then(|n| n.as_str()) {
                if is_azure_managed_rg(name) {
                    return false;
                }
            }
            match rg.get("tags").and_then(|t| t.get(tag_name)) {
                Some(v) => match (v.as_str(), tag_value) {
                    (Some(s), Some(want)) => s == want,
                    (Some(_), None) => true,
                    _ => false,
                },
                None => false,
            }
        })
        .collect()
}

fn is_azure_managed_rg(name: &str) -> bool {
    name.starts_with("MA_")
        || name.starts_with("MC_")
        || name.starts_with("AzureBackupRG_")
        || name.starts_with("NetworkWatcherRG")
        || name.starts_with("databricks-rg-")
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

    #[test]
    fn filter_rgs_by_tag_excludes_azure_managed_rgs_even_when_tagged() {
        let rgs = vec![
            rg(
                "rg-azcluster-demo",
                json!({"azcluster:managed": "true", "azcluster:name": "demo"}),
            ),
            rg(
                "MA_amw-demo_eastus_managed",
                json!({"azcluster:managed": "true", "azcluster:name": "demo"}),
            ),
            rg(
                "MC_demo_aks_eastus",
                json!({"azcluster:managed": "true", "azcluster:name": "demo"}),
            ),
            rg(
                "NetworkWatcherRG",
                json!({"azcluster:managed": "true", "azcluster:name": "demo"}),
            ),
        ];
        let by_name = filter_rgs_by_tag(rgs, "azcluster:name", Some("demo"));
        assert_eq!(by_name.len(), 1);
        assert_eq!(
            by_name[0].get("name").and_then(|v| v.as_str()),
            Some("rg-azcluster-demo")
        );
    }

    #[test]
    fn is_azure_managed_rg_recognises_known_prefixes() {
        assert!(is_azure_managed_rg("MA_amw_eastus_managed"));
        assert!(is_azure_managed_rg("MC_aks_cluster_eastus"));
        assert!(is_azure_managed_rg("AzureBackupRG_eastus_1"));
        assert!(is_azure_managed_rg("NetworkWatcherRG"));
        assert!(is_azure_managed_rg("databricks-rg-workspace-id"));
        assert!(!is_azure_managed_rg("rg-azcluster-demo"));
        assert!(!is_azure_managed_rg("rg-ma-confusing"));
    }

    #[test]
    fn nested_module_target_falls_back_to_id_when_resource_group_field_absent() {
        let op = json!({
            "properties": {
                "targetResource": {
                    "id": "/subscriptions/SUB/resourceGroups/rg-azcluster-demo/providers/Microsoft.Resources/deployments/cluster-demo",
                    "resourceType": "Microsoft.Resources/deployments",
                    "resourceName": "cluster-demo"
                }
            }
        });
        assert_eq!(
            super::nested_module_target(&op),
            Some(("rg-azcluster-demo".to_string(), "cluster-demo".to_string()))
        );
    }

    #[test]
    fn nested_module_target_sub_scope_has_empty_rg() {
        let op = json!({
            "properties": {
                "targetResource": {
                    "id": "/subscriptions/SUB/providers/Microsoft.Resources/deployments/sub-nested",
                    "resourceType": "Microsoft.Resources/deployments",
                    "resourceName": "sub-nested"
                }
            }
        });
        assert_eq!(
            super::nested_module_target(&op),
            Some((String::new(), "sub-nested".to_string()))
        );
    }

    #[test]
    fn nested_module_target_non_deployment_returns_none() {
        let op = json!({
            "properties": {
                "targetResource": {
                    "id": "/subscriptions/SUB/resourceGroups/rg-x/providers/Microsoft.Compute/virtualMachines/vm-x",
                    "resourceType": "Microsoft.Compute/virtualMachines",
                    "resourceName": "vm-x"
                }
            }
        });
        assert_eq!(super::nested_module_target(&op), None);
    }

    #[test]
    fn dedup_ops_by_target_keeps_latest_state_per_id_preserves_first_seen_order() {
        let running = |id: &str| {
            json!({
                "properties": {
                    "provisioningState": "Running",
                    "targetResource": {"id": id, "resourceType": "X", "resourceName": "n"}
                }
            })
        };
        let succeeded = |id: &str| {
            json!({
                "properties": {
                    "provisioningState": "Succeeded",
                    "targetResource": {"id": id, "resourceType": "X", "resourceName": "n"}
                }
            })
        };
        let ops = vec![
            running("/A"),
            running("/B"),
            succeeded("/A"),
            succeeded("/B"),
        ];
        let deduped = super::dedup_ops_by_target(ops);
        assert_eq!(deduped.len(), 2);
        assert_eq!(
            deduped[0].pointer("/properties/targetResource/id"),
            Some(&json!("/A"))
        );
        assert_eq!(
            deduped[0].pointer("/properties/provisioningState"),
            Some(&json!("Succeeded"))
        );
        assert_eq!(
            deduped[1].pointer("/properties/targetResource/id"),
            Some(&json!("/B"))
        );
        assert_eq!(
            deduped[1].pointer("/properties/provisioningState"),
            Some(&json!("Succeeded"))
        );
    }

    #[test]
    fn dedup_ops_by_target_drops_ops_without_target_id() {
        let ops = vec![
            json!({"properties": {"provisioningState": "Running", "targetResource": null}}),
            json!({"properties": {"provisioningState": "Succeeded"}}),
        ];
        let deduped = super::dedup_ops_by_target(ops);
        assert!(deduped.is_empty());
    }

    #[test]
    fn run_command_result_parses_terminal_success_with_nonzero_exit() {
        let v = json!({
            "properties": {
                "provisioningState": "Succeeded",
                "exitCode": 1,
                "logs": "helm: release already exists\n"
            }
        });
        let r = super::parse_run_command_result(&v).expect("parsed");
        assert_eq!(r.provisioning_state, "Succeeded");
        assert_eq!(r.exit_code, 1);
        assert!(r.logs.contains("already exists"));
    }

    #[test]
    fn run_command_result_in_progress_has_no_exit_code() {
        let v = json!({ "properties": { "provisioningState": "Running" } });
        let r = super::parse_run_command_result(&v).expect("parsed");
        assert_eq!(r.provisioning_state, "Running");
        assert_eq!(r.exit_code, -1);
        assert!(r.logs.is_empty());
    }

    #[test]
    fn run_command_result_none_without_provisioning_state() {
        let v = json!({ "properties": { "exitCode": 0 } });
        assert!(super::parse_run_command_result(&v).is_none());
        assert!(super::parse_run_command_result(&json!({})).is_none());
    }
}
