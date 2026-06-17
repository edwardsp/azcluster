use crate::aks::{operators, output_string, output_u32};
use crate::cluster_state::{AksState, ClusterSecrets, ClusterState, PendingDeploy, Target};
use crate::{crypto, deploy_progress, DeployArgs, PoolSpec};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

pub(crate) fn deploy_aks(args: DeployArgs) -> Result<()> {
    let template = crate::resolve_aks_template(args.template.clone())?;
    let sub_id = crate::current_subscription_id()?;
    let key_vault_name = crypto::derive_kv_name(&sub_id, &args.name, &args.location);
    let (deployer_oid, deployer_ptype) = crate::current_principal()?;
    let resolved_rg = args
        .resource_group
        .clone()
        .unwrap_or_else(|| format!("rg-azcluster-{}", args.name));

    let gpu_pool = select_gpu_pool(&args)?;
    let gpu_pool_name = sanitize_agent_pool_name(&gpu_pool.name)?;
    let storage_enabled = args.storage && !args.no_storage;
    let storage_account_name = if storage_enabled {
        match args.storage_name.as_deref() {
            Some(name) => {
                crypto::validate_storage_account_name(name)?;
                name.to_string()
            }
            None => crypto::derive_storage_account_name(&sub_id, &args.name, &args.location),
        }
    } else {
        String::new()
    };
    if storage_enabled {
        eprintln!("==> [aks] per-cluster Blob storage account: {storage_account_name}");
    }
    let existing_secrets = ClusterSecrets::load_optional(&args.name)?;
    if existing_secrets.is_some() {
        eprintln!(
            "==> [aks] reusing persisted secrets for cluster '{}' (re-invocation safe)",
            args.name
        );
    }
    let secrets = ensure_admin_secrets(&args.name, existing_secrets.as_ref())?;
    let secrets_path = secrets.save(&args.name)?;
    eprintln!(
        "==> [aks] saved cluster secrets -> {}",
        secrets_path.display()
    );
    eprintln!("==> [aks] per-cluster Key Vault: {key_vault_name}");

    let client = crate::arm_client()?;
    crate::aks::feature::ensure_ib_feature_registered(&client)?;

    let deployment_name = if args.skip_arm {
        let pending = PendingDeploy::load_optional(&args.name)?.ok_or_else(|| {
            anyhow!(
                "--skip-arm: no pending deploy marker for '{}' (~/.config/azcluster/clusters/{}-pending.toml)",
                args.name,
                args.name
            )
        })?;
        eprintln!(
            "==> [aks] --skip-arm: reusing pending deployment_name '{}'",
            pending.deployment_name
        );
        pending.deployment_name
    } else {
        format!("azcluster-aks-{}-{}", args.name, crate::utc_stamp())
    };

    let params_json = build_params(
        &args,
        &AksParams {
            gpu_pool: &gpu_pool,
            gpu_pool_name: &gpu_pool_name,
            ssh_public_key: &secrets.admin_ssh_public_key,
            deployer_oid: &deployer_oid,
            deployer_ptype: &deployer_ptype,
            key_vault_name: &key_vault_name,
            storage_enabled,
            storage_account_name: &storage_account_name,
        },
    );

    if args.what_if {
        eprintln!("==> [aks] ARM whatIf deployment '{}'", deployment_name);
        let result = client
            .whatif_subscription_deployment(&deployment_name, &args.location, template, params_json)
            .context("AKS whatIf submission failed")?;
        let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        println!("{pretty}");
        return Ok(());
    }

    let pending = PendingDeploy {
        cluster: args.name.clone(),
        deployment_name: deployment_name.clone(),
        resource_group: resolved_rg.clone(),
        started_at: crate::utc_iso8601(),
        monitoring_enabled: false,
        accounting_enabled: false,
        shared_storage: "aks".into(),
        grafana_location: args.grafana_location.clone(),
        extra_packages: Vec::new(),
        bastion_enabled: false,
        storage_enabled: false,
        storage_account_name: None,
        storage_hns: false,
        storage_public_access: false,
        azcp_version: None,
    };
    if !args.skip_arm {
        let pending_path = pending.save()?;
        eprintln!(
            "==> [aks] saved pending deploy -> {}",
            pending_path.display()
        );
    }

    if !args.skip_arm {
        eprintln!(
            "==> [aks] ARM create deployment '{}'{}",
            deployment_name,
            if args.no_wait { " (--no-wait)" } else { "" }
        );
        client
            .create_subscription_deployment(&deployment_name, &args.location, template, params_json)
            .context("AKS ARM deployment submission failed")?;
    } else {
        eprintln!("==> [aks] --skip-arm: bypassing ARM submission");
    }

    if args.no_wait {
        eprintln!(
            "==> [aks] ARM deployment '{}' submitted. Re-run `azcluster deploy --target aks --name {} --location {} --skip-arm --pool ...` after it succeeds to finalize.",
            deployment_name, args.name, args.location
        );
        return Ok(());
    }

    if !args.skip_arm {
        eprintln!(
            "==> [aks] waiting for ARM deployment '{}' to complete...",
            deployment_name
        );
        let mut progress = deploy_progress::Renderer::new();
        let final_state = client
            .wait_for_deployment_completion_with_progress(&deployment_name, &mut |ops| {
                progress.render(ops);
            })
            .context("polling AKS ARM deployment")?;
        progress.finish();
        let state_str = final_state
            .get("properties")
            .and_then(|p| p.get("provisioningState"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if state_str != "Succeeded" {
            bail!(
                "AKS ARM deployment '{}' ended in state {state_str}. Run `azcluster delete {}` to tear down.",
                deployment_name,
                args.name
            );
        }
    }

    let deployment = client.get_deployment(&deployment_name)?;
    let outputs = deployment
        .get("properties")
        .and_then(|p| p.get("outputs"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let aks_cluster_name = require_output(&outputs, "aksClusterName")?;
    let cluster_rg = output_string(&outputs, "resourceGroupName")?.unwrap_or(resolved_rg.clone());
    let gpu_node_count = output_u32(&outputs, "gpuNodeCount")?;
    let stages_completed = operators::install_all(
        &client,
        &cluster_rg,
        &aks_cluster_name,
        gpu_node_count,
        storage_enabled,
    )?;

    finalize_deploy_aks(
        &args,
        &deployment_name,
        &cluster_rg,
        &sub_id,
        &outputs,
        &secrets,
        stages_completed,
    )?;
    PendingDeploy::delete(&args.name)?;
    Ok(())
}

fn select_gpu_pool(args: &DeployArgs) -> Result<PoolSpec> {
    if args.pools.is_empty() {
        bail!("--target aks requires at least one --pool name=...,sku=...,count=N[,default]");
    }
    let pools = args
        .pools
        .iter()
        .map(|s| crate::parse_pool(s))
        .collect::<Result<Vec<_>>>()?;
    pools
        .iter()
        .find(|p| p.is_default)
        .or_else(|| pools.first())
        .cloned()
        .ok_or_else(|| anyhow!("--target aks requires at least one --pool"))
}

fn sanitize_agent_pool_name(name: &str) -> Result<String> {
    let out: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .take(12)
        .collect();
    if out.is_empty() {
        bail!("AKS GPU pool name '{name}' has no lowercase alphanumeric characters");
    }
    Ok(out)
}

fn ensure_admin_secrets(name: &str, existing: Option<&ClusterSecrets>) -> Result<ClusterSecrets> {
    if let Some(s) = existing {
        if !s.admin_ssh_public_key.is_empty() && !s.admin_ssh_private_key.is_empty() {
            return Ok(s.clone());
        }
    }
    let kp = crypto::generate_admin_keypair(&format!("azcluster-{name}"))?;
    eprintln!("==> [aks] generated fresh ed25519 admin keypair for cluster '{name}'");
    Ok(ClusterSecrets {
        ldap_admin_password: existing
            .map(|s| s.ldap_admin_password.clone())
            .unwrap_or_default(),
        mysql_admin_password: existing.and_then(|s| s.mysql_admin_password.clone()),
        admin_ssh_public_key: kp.public_openssh,
        admin_ssh_private_key: kp.private_openssh_pem,
    })
}

struct AksParams<'a> {
    gpu_pool: &'a PoolSpec,
    gpu_pool_name: &'a str,
    ssh_public_key: &'a str,
    deployer_oid: &'a str,
    deployer_ptype: &'a str,
    key_vault_name: &'a str,
    storage_enabled: bool,
    storage_account_name: &'a str,
}

fn build_params(args: &DeployArgs, p: &AksParams) -> Value {
    let params: Vec<(&str, Value)> = vec![
        ("clusterName", json!(args.name)),
        ("location", json!(args.location)),
        (
            "existingResourceGroup",
            json!(args.resource_group.clone().unwrap_or_default()),
        ),
        ("azclusterVersion", json!(args.azcluster_version)),
        ("kubernetesVersion", json!("")),
        ("systemNodeSku", json!("Standard_D8s_v5")),
        ("systemNodeCount", json!(2)),
        ("gpuPoolName", json!(p.gpu_pool_name)),
        ("gpuSku", json!(p.gpu_pool.sku)),
        ("gpuNodeCount", json!(p.gpu_pool.count)),
        ("vnetAddressPrefix", json!("10.42.0.0/16")),
        ("sshPublicKey", json!(p.ssh_public_key.trim())),
        ("adminUsername", json!("azureuser")),
        ("deployerPrincipalId", json!(p.deployer_oid)),
        ("deployerPrincipalType", json!(p.deployer_ptype)),
        ("keyVaultName", json!(p.key_vault_name)),
        ("enableMonitoring", json!(false)),
        (
            "grafanaLocation",
            json!(args
                .grafana_location
                .clone()
                .unwrap_or_else(|| args.location.clone())),
        ),
        ("enableStorage", json!(p.storage_enabled)),
        ("storageAccountName", json!(p.storage_account_name)),
        ("storageSku", json!(args.storage_sku)),
        ("storageAccessTier", json!(args.storage_tier)),
    ];
    let mut params_obj = serde_json::Map::new();
    for (k, v) in params {
        params_obj.insert(k.to_string(), json!({ "value": v }));
    }
    Value::Object(params_obj)
}

fn require_output(outputs: &Value, key: &str) -> Result<String> {
    output_string(outputs, key)?.ok_or_else(|| anyhow!("deployment did not return {key}"))
}

fn finalize_deploy_aks(
    args: &DeployArgs,
    deployment_name: &str,
    resource_group: &str,
    sub_id: &str,
    outputs: &Value,
    secrets: &ClusterSecrets,
    stages_completed: Vec<String>,
) -> Result<()> {
    let kv_name = require_output(outputs, "keyVaultName")?;
    let state = ClusterState {
        name: args.name.clone(),
        subscription_id: sub_id.to_string(),
        resource_group: resource_group.to_string(),
        location: args.location.clone(),
        target: Target::Aks,
        admin_username: "azureuser".into(),
        login_public_ip: None,
        scheduler_private_ip: String::new(),
        anf_mount_ip: None,
        compute_vmss_names: Vec::new(),
        extra_packages: Vec::new(),
        accounting_enabled: false,
        bastion_enabled: false,
        bastion_name: None,
        bastion_dns_name: None,
        bastion_resource_id: None,
        storage_enabled: output_string(outputs, "storageAccountName")?
            .is_some_and(|s| !s.is_empty()),
        storage_account_name: output_string(outputs, "storageAccountName")?
            .filter(|s| !s.is_empty()),
        storage_blob_endpoint: output_string(outputs, "storageBlobEndpoint")?
            .filter(|s| !s.is_empty()),
        storage_dfs_endpoint: None,
        storage_data_container_url: output_string(outputs, "storageDataContainerUrl")?
            .filter(|s| !s.is_empty()),
        storage_hns: false,
        storage_public_access: false,
        azcp_version: None,
        aks: Some(AksState {
            aks_cluster_name: require_output(outputs, "aksClusterName")?,
            node_resource_group: require_output(outputs, "nodeResourceGroup")?,
            fqdn: output_string(outputs, "fqdn")?.filter(|s| !s.is_empty()),
            gpu_pool_name: require_output(outputs, "gpuPoolName")?,
            gpu_sku: require_output(outputs, "gpuSku")?,
            gpu_node_count: output_u32(outputs, "gpuNodeCount")?,
            kubelet_identity_object_id: output_string(outputs, "kubeletIdentityObjectId")?
                .filter(|s| !s.is_empty()),
            oidc_issuer_url: output_string(outputs, "oidcIssuerUrl")?.filter(|s| !s.is_empty()),
            stages_completed,
        }),
    };
    let saved = state.save()?;
    eprintln!("==> [aks] saved cluster state -> {}", saved.display());
    let secrets_path = secrets.save(&args.name)?;
    eprintln!(
        "==> [aks] saved cluster secrets -> {}",
        secrets_path.display()
    );

    match crate::upload_cluster_to_keyvault(&kv_name, &state, secrets) {
        Ok(()) => eprintln!(
            "==> [aks] uploaded cluster manifest + secrets bundle to Key Vault '{kv_name}'"
        ),
        Err(e) => eprintln!(
            "==> [aks] WARNING: Key Vault upload to '{kv_name}' failed: {e:#}. Local state intact; re-run `azcluster deploy --target aks --name {}` to retry.",
            args.name
        ),
    }
    match crate::tag_resource_group_for_cluster_target(
        &crate::arm_client()?,
        &state.resource_group,
        &args.name,
        &kv_name,
        &args.azcluster_version,
        Some("aks"),
    ) {
        Ok(()) => eprintln!(
            "==> [aks] tagged RG '{}' with azcluster:* discovery tags",
            state.resource_group
        ),
        Err(e) => eprintln!(
            "==> [aks] WARNING: RG tag PATCH on '{}' failed: {e:#}. Cluster will be invisible to `azcluster list`; re-run deploy to retry.",
            state.resource_group
        ),
    }
    if let Err(e) = crate::timings::capture(
        &crate::arm_client()?,
        &args.name,
        deployment_name,
        &state.resource_group,
        "aks",
    ) {
        eprintln!("==> [aks] warning: timing capture failed: {e:#}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_agent_pool_name() {
        assert_eq!(
            sanitize_agent_pool_name("gpu-prod_01").unwrap(),
            "gpuprod01"
        );
        assert_eq!(
            sanitize_agent_pool_name("ABCDEFGHIJKLMN").unwrap(),
            "abcdefghijkl"
        );
        assert!(sanitize_agent_pool_name("---").is_err());
    }
}
