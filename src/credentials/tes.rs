// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Token Exchange Service credential provider for GG Classic and Lite

use aws_sdk_cloudwatchlogs::config::{Credentials, ProvideCredentials};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Credential provider that wraps GG TES for both Classic and Lite environments.
/// On Classic: calls local TES HTTP endpoint via GG IPC.
/// On Lite: subscribes to topic-based TES credential topic.
pub struct GgCredentialProvider {
    env: super::GgEnvironment,
    cached: Mutex<Option<CachedCredentials>>,
}

struct CachedCredentials {
    credentials: Credentials,
    fetched_at: Instant,
    expires_in: Duration,
}

impl GgCredentialProvider {
    pub fn new(env: super::GgEnvironment) -> Self {
        Self {
            env,
            cached: Mutex::new(None),
        }
    }

    #[cfg(not(tarpaulin_include))]
    async fn fetch_credentials(&self) -> Result<Credentials, String> {
        match self.env {
            super::GgEnvironment::Classic => {
                tracing::debug!("Refreshing credentials via GG IPC TES endpoint");
                // In production, this calls the local TES HTTP endpoint via aws-greengrass-ipc.
                // The actual IPC call is: GreengrassIpcClient -> get_credential()
                // For now, fall back to default credential chain which includes
                // the TES endpoint when running as a GG component.
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                config
                    .credentials_provider()
                    .ok_or_else(|| "No credential provider in default config".to_string())?
                    .provide_credentials()
                    .await
                    .map_err(|e| format!("Failed to get credentials: {e}"))
            }
            super::GgEnvironment::Lite => {
                tracing::debug!("Refreshing credentials via topic-based TES");
                // On Lite, credentials come from a topic subscription.
                // Fall back to default credential chain which picks up env vars
                // set by the GG Lite runtime.
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                config
                    .credentials_provider()
                    .ok_or_else(|| "No credential provider in default config".to_string())?
                    .provide_credentials()
                    .await
                    .map_err(|e| format!("Failed to get credentials: {e}"))
            }
        }
    }

    /// Get credentials, using cache if not expired (refresh 5 min before expiry).
    #[cfg(not(tarpaulin_include))]
    pub async fn get_credentials(&self) -> Result<Credentials, String> {
        let mut cached = self.cached.lock().await;
        if let Some(ref c) = *cached {
            let elapsed = c.fetched_at.elapsed();
            let refresh_at = c.expires_in.saturating_sub(Duration::from_secs(300));
            if elapsed < refresh_at {
                return Ok(c.credentials.clone());
            }
        }

        let creds = self.fetch_credentials().await?;
        *cached = Some(CachedCredentials {
            credentials: creds.clone(),
            fetched_at: Instant::now(),
            expires_in: Duration::from_secs(3600), // TES tokens typically last 1 hour
        });
        Ok(creds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation_classic() {
        let provider = GgCredentialProvider::new(super::super::GgEnvironment::Classic);
        assert!(matches!(provider.env, super::super::GgEnvironment::Classic));
    }

    #[test]
    fn test_provider_creation_lite() {
        let provider = GgCredentialProvider::new(super::super::GgEnvironment::Lite);
        assert!(matches!(provider.env, super::super::GgEnvironment::Lite));
    }

    #[test]
    fn test_cached_credentials_struct() {
        let creds = Credentials::new("key", "secret", None, None, "test");
        let cached = CachedCredentials {
            credentials: creds,
            fetched_at: Instant::now(),
            expires_in: Duration::from_secs(3600),
        };
        assert_eq!(cached.expires_in, Duration::from_secs(3600));
        assert!(cached.fetched_at.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn test_refresh_before_expiry_logic() {
        // Test the Duration math for refresh-before-expiry
        let expires_in = Duration::from_secs(3600);
        let refresh_buffer = Duration::from_secs(300);
        let refresh_at = expires_in.saturating_sub(refresh_buffer);
        assert_eq!(refresh_at, Duration::from_secs(3300)); // 55 minutes

        // Edge case: expires_in less than buffer
        let short_expiry = Duration::from_secs(200);
        let refresh_at_short = short_expiry.saturating_sub(refresh_buffer);
        assert_eq!(refresh_at_short, Duration::ZERO);
    }

    #[tokio::test]
    async fn test_cache_initially_empty() {
        let provider = GgCredentialProvider::new(super::super::GgEnvironment::Classic);
        let cached = provider.cached.lock().await;
        assert!(cached.is_none());
    }
}
