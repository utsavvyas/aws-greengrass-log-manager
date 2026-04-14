// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Credential provider and environment detection for GG Classic and Lite

mod tes;

/// GG runtime environment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GgEnvironment {
    Classic,
    Lite,
}

/// Detect GG environment by checking for IPC socket.
/// Classic: `$AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT` is set.
/// Lite: env var is absent (no IPC socket).
pub fn detect_environment() -> GgEnvironment {
    let env = if std::env::var("AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT").is_ok() {
        GgEnvironment::Classic
    } else {
        GgEnvironment::Lite
    };
    tracing::info!(environment = ?env, "Detected GG environment");
    env
}

/// Resolve ThingName from environment.
/// Classic: `$AWS_IOT_THING_NAME` env var.
/// Lite: `$GG_THING_NAME` env var.
pub fn resolve_thing_name(env: GgEnvironment) -> Option<String> {
    let thing_name = match env {
        GgEnvironment::Classic => std::env::var("AWS_IOT_THING_NAME").ok(),
        GgEnvironment::Lite => std::env::var("GG_THING_NAME").ok(),
    };
    tracing::debug!(thing_name = ?thing_name, "Resolved thing name");
    thing_name
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn test_detect_classic() {
        std::env::set_var(
            "AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT",
            "/tmp/ipc.sock",
        );
        assert_eq!(detect_environment(), GgEnvironment::Classic);
        std::env::remove_var("AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT");
    }

    #[test]
    #[serial]
    fn test_detect_lite() {
        std::env::remove_var("AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT");
        assert_eq!(detect_environment(), GgEnvironment::Lite);
    }

    #[test]
    #[serial]
    fn test_resolve_thing_name_classic() {
        std::env::set_var("AWS_IOT_THING_NAME", "test-device-01");
        assert_eq!(
            resolve_thing_name(GgEnvironment::Classic),
            Some("test-device-01".to_string())
        );
        std::env::remove_var("AWS_IOT_THING_NAME");
    }

    #[test]
    #[serial]
    fn test_resolve_thing_name_lite() {
        std::env::set_var("GG_THING_NAME", "lite-device-01");
        assert_eq!(
            resolve_thing_name(GgEnvironment::Lite),
            Some("lite-device-01".to_string())
        );
        std::env::remove_var("GG_THING_NAME");
    }

    #[test]
    #[serial]
    fn test_resolve_thing_name_missing() {
        std::env::remove_var("AWS_IOT_THING_NAME");
        std::env::remove_var("GG_THING_NAME");
        assert_eq!(resolve_thing_name(GgEnvironment::Classic), None);
        assert_eq!(resolve_thing_name(GgEnvironment::Lite), None);
    }
}
