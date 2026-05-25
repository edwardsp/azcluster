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
}

impl Default for ApiVersions {
    fn default() -> Self {
        Self {
            resource_group: "2024-03-01".to_string(),
            deployment: "2024-03-01".to_string(),
            compute: "2024-07-01".to_string(),
            network: "2023-11-01".to_string(),
            storage: "2023-05-01".to_string(),
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

    /// Create a subscription-level deployment.
    pub fn create_deployment(
        &self,
        deployment_name: &str,
        template: Value,
        parameters: Value,
    ) -> Result<Value> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Resources/deployments/{}?api-version={}",
            self.subscription_id, deployment_name, self.api_versions.deployment
        );

        let body = json!({
            "properties": {
                "template": template,
                "parameters": parameters,
                "mode": "Incremental",
            }
        });

        self.put(&url, body)
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

    /// List deployment operations (for nested deployments).
    pub fn list_deployment_operations(&self, deployment_name: &str) -> Result<Vec<Value>> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/providers/Microsoft.Resources/deployments/{}/operations?api-version={}",
            self.subscription_id, deployment_name, self.api_versions.deployment
        );
        self.list_paginated(&url)
    }

    /// Get deployment operations with timing information.
    pub fn get_deployment_operations_with_timings(
        &self,
        deployment_name: &str,
    ) -> Result<Vec<Value>> {
        let operations = self.list_deployment_operations(deployment_name)?;

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
}
