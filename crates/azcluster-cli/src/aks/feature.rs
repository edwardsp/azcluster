use crate::arm::client::ArmClient;
use anyhow::{anyhow, Result};
use std::time::{Duration, Instant};

pub const AKS_PROVIDER: &str = "Microsoft.ContainerService";
pub const IB_FEATURE: &str = "AKSInfinibandSupport";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeatureAction {
    AlreadyRegistered,
    Register,
    WaitForPropagation,
}

fn classify_feature_state(state: &str) -> FeatureAction {
    match state {
        "Registered" => FeatureAction::AlreadyRegistered,
        "Registering" | "Pending" => FeatureAction::WaitForPropagation,
        "NotRegistered" => FeatureAction::Register,
        _ => FeatureAction::Register,
    }
}

pub fn ensure_ib_feature_registered(arm: &ArmClient) -> Result<()> {
    let initial_state = feature_state(arm)?;
    match classify_feature_state(&initial_state) {
        FeatureAction::AlreadyRegistered => {
            eprintln!("==> [aks] InfiniBand feature {IB_FEATURE} already Registered");
            arm.register_provider(AKS_PROVIDER)?;
            return Ok(());
        }
        FeatureAction::Register => {
            eprintln!("==> [aks] registering InfiniBand feature {IB_FEATURE}");
            arm.register_feature(AKS_PROVIDER, IB_FEATURE)?;
        }
        FeatureAction::WaitForPropagation => {
            eprintln!("==> [aks] InfiniBand feature {IB_FEATURE} is {initial_state}; waiting");
        }
    }

    const MAX_WAIT: Duration = Duration::from_secs(20 * 60);
    const POLL: Duration = Duration::from_secs(15);
    let started = Instant::now();
    loop {
        let state = feature_state(arm)?;
        eprintln!("==> [aks] {IB_FEATURE} state: {state}");
        if classify_feature_state(&state) == FeatureAction::AlreadyRegistered {
            eprintln!("==> [aks] refreshing provider registration for {AKS_PROVIDER}");
            arm.register_provider(AKS_PROVIDER)?;
            return Ok(());
        }
        if started.elapsed() > MAX_WAIT {
            return Err(anyhow!(
                "{IB_FEATURE} did not reach Registered within 20 min (last state: {state})"
            ));
        }
        std::thread::sleep(POLL);
    }
}

fn feature_state(arm: &ArmClient) -> Result<String> {
    let v = arm.get_feature(AKS_PROVIDER, IB_FEATURE)?;
    Ok(v.pointer("/properties/state")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_registered_skips() {
        assert_eq!(
            classify_feature_state("Registered"),
            FeatureAction::AlreadyRegistered
        );
    }

    #[test]
    fn classify_in_progress_waits() {
        assert_eq!(
            classify_feature_state("Registering"),
            FeatureAction::WaitForPropagation
        );
        assert_eq!(
            classify_feature_state("Pending"),
            FeatureAction::WaitForPropagation
        );
    }

    #[test]
    fn classify_not_registered_or_unknown_registers() {
        assert_eq!(
            classify_feature_state("NotRegistered"),
            FeatureAction::Register
        );
        assert_eq!(classify_feature_state("Failed"), FeatureAction::Register);
        assert_eq!(classify_feature_state(""), FeatureAction::Register);
    }
}
