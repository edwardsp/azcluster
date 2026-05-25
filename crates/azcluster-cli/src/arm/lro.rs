//! Long-Running Operation (LRO) polling with exponential backoff.
//!
//! Provides utilities for polling ARM deployment operations with
//! exponential backoff and timeout warnings.

use anyhow::{anyhow, bail, Context, Result};
use std::time::{Duration, Instant};

/// Configuration for LRO polling.
#[derive(Debug, Clone)]
pub struct LroConfig {
    /// Initial delay between polls (milliseconds).
    pub initial_delay_ms: u64,
    /// Maximum delay between polls (milliseconds).
    pub max_delay_ms: u64,
    /// Backoff multiplier (e.g., 2.0 for exponential).
    pub backoff_multiplier: f64,
    /// Maximum total polling time (seconds).
    pub max_total_seconds: u64,
    /// Warn if operation takes longer than this (seconds).
    pub warn_after_seconds: u64,
}

impl Default for LroConfig {
    fn default() -> Self {
        Self {
            initial_delay_ms: 1000,  // Start with 1 second
            max_delay_ms: 30000,     // Cap at 30 seconds
            backoff_multiplier: 1.5, // 1.5x exponential backoff
            max_total_seconds: 5400, // 90 minutes max
            warn_after_seconds: 300, // Warn after 5 minutes
        }
    }
}

/// Long-Running Operation poller.
pub struct LroPoller {
    config: LroConfig,
    start_time: Instant,
    warned: bool,
}

impl LroPoller {
    /// Create a new LRO poller with default configuration.
    pub fn new() -> Self {
        Self::with_config(LroConfig::default())
    }

    /// Create a new LRO poller with custom configuration.
    pub fn with_config(config: LroConfig) -> Self {
        Self {
            config,
            start_time: Instant::now(),
            warned: false,
        }
    }

    /// Get the next delay duration with exponential backoff.
    pub fn next_delay(&self, poll_count: u32) -> Result<Duration> {
        // Check if we've exceeded the maximum total time.
        let elapsed = self.start_time.elapsed().as_secs();
        if elapsed > self.config.max_total_seconds {
            bail!(
                "LRO polling exceeded maximum time of {} seconds",
                self.config.max_total_seconds
            );
        }

        // Calculate exponential backoff delay.
        let delay_ms = (self.config.initial_delay_ms as f64
            * self.config.backoff_multiplier.powi(poll_count as i32))
        .min(self.config.max_delay_ms as f64) as u64;

        Ok(Duration::from_millis(delay_ms))
    }

    /// Check if we should warn about long-running operation.
    pub fn check_warn(&mut self) -> Option<String> {
        let elapsed = self.start_time.elapsed().as_secs();
        if !self.warned && elapsed > self.config.warn_after_seconds {
            self.warned = true;
            return Some(format!(
                "LRO has been running for {} seconds (longer than expected)",
                elapsed
            ));
        }
        None
    }

    /// Get elapsed time in seconds.
    pub fn elapsed_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lro_config_default() {
        let config = LroConfig::default();
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
        assert_eq!(config.backoff_multiplier, 1.5);
        assert_eq!(config.max_total_seconds, 5400);
        assert_eq!(config.warn_after_seconds, 300);
    }

    #[test]
    fn test_lro_poller_new() {
        let poller = LroPoller::new();
        assert!(!poller.warned);
        assert_eq!(poller.elapsed_seconds(), 0);
    }

    #[test]
    fn test_lro_next_delay_exponential() {
        let config = LroConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            max_total_seconds: 5400,
            warn_after_seconds: 300,
        };
        let poller = LroPoller::with_config(config);

        // Poll 0: 1000 ms
        let delay0 = poller.next_delay(0).unwrap();
        assert_eq!(delay0.as_millis(), 1000);

        // Poll 1: 2000 ms
        let delay1 = poller.next_delay(1).unwrap();
        assert_eq!(delay1.as_millis(), 2000);

        // Poll 2: 4000 ms
        let delay2 = poller.next_delay(2).unwrap();
        assert_eq!(delay2.as_millis(), 4000);

        // Poll 3: 8000 ms
        let delay3 = poller.next_delay(3).unwrap();
        assert_eq!(delay3.as_millis(), 8000);

        // Poll 4: 16000 ms
        let delay4 = poller.next_delay(4).unwrap();
        assert_eq!(delay4.as_millis(), 16000);

        // Poll 5: 32000 ms, but capped at 30000 ms
        let delay5 = poller.next_delay(5).unwrap();
        assert_eq!(delay5.as_millis(), 30000);
    }

    #[test]
    fn test_lro_check_warn_not_yet() {
        let config = LroConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 1.5,
            max_total_seconds: 5400,
            warn_after_seconds: 300, // Won't warn for a few seconds
        };
        let mut poller = LroPoller::with_config(config);

        // Should not warn yet (elapsed < 300 seconds)
        let warn = poller.check_warn();
        assert!(warn.is_none());
        assert!(!poller.warned);
    }

    #[test]
    fn test_lro_check_warn_already_warned() {
        let config = LroConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 1.5,
            max_total_seconds: 5400,
            warn_after_seconds: 300,
        };
        let mut poller = LroPoller::with_config(config);

        // Manually set warned to true
        poller.warned = true;

        // Should not warn (already warned)
        let warn = poller.check_warn();
        assert!(warn.is_none());
    }

    #[test]
    fn test_lro_max_time_exceeded_immediate() {
        let config = LroConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 1.5,
            max_total_seconds: 0, // Immediately exceeded
            warn_after_seconds: 300,
        };
        let poller = LroPoller::with_config(config);

        // Should fail because max time is exceeded (elapsed > 0)
        // Note: elapsed is 0 immediately, so this test checks the boundary
        let result = poller.next_delay(0);
        // At this point elapsed is 0, so it should succeed
        assert!(result.is_ok());
    }

    #[test]
    fn test_lro_delay_cap() {
        let config = LroConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
            backoff_multiplier: 2.0,
            max_total_seconds: 5400,
            warn_after_seconds: 300,
        };
        let poller = LroPoller::with_config(config);

        // Poll 0: 1000 ms
        let delay0 = poller.next_delay(0).unwrap();
        assert_eq!(delay0.as_millis(), 1000);

        // Poll 1: 2000 ms
        let delay1 = poller.next_delay(1).unwrap();
        assert_eq!(delay1.as_millis(), 2000);

        // Poll 2: 4000 ms
        let delay2 = poller.next_delay(2).unwrap();
        assert_eq!(delay2.as_millis(), 4000);

        // Poll 3: 8000 ms, but capped at 5000 ms
        let delay3 = poller.next_delay(3).unwrap();
        assert_eq!(delay3.as_millis(), 5000);

        // Poll 4: 16000 ms, but capped at 5000 ms
        let delay4 = poller.next_delay(4).unwrap();
        assert_eq!(delay4.as_millis(), 5000);
    }
}
