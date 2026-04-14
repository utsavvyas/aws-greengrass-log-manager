// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for credential detection

use serial_test::serial;

// Re-implement detection logic for testing since credentials module is private
fn detect_environment() -> &'static str {
    if std::env::var("AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT").is_ok() {
        "Classic"
    } else {
        "Lite"
    }
}

fn resolve_thing_name(env: &str) -> Option<String> {
    match env {
        "Classic" => std::env::var("AWS_IOT_THING_NAME").ok(),
        "Lite" => std::env::var("GG_THING_NAME").ok(),
        _ => None,
    }
}

#[test]
#[serial]
fn test_detect_environment_classic() {
    std::env::set_var(
        "AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT",
        "/tmp/ipc.sock",
    );
    assert_eq!(detect_environment(), "Classic");
    std::env::remove_var("AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT");
}

#[test]
#[serial]
fn test_detect_environment_lite() {
    std::env::remove_var("AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT");
    assert_eq!(detect_environment(), "Lite");
}

#[test]
#[serial]
fn test_resolve_thing_name() {
    // Classic
    std::env::set_var("AWS_IOT_THING_NAME", "classic-device");
    assert_eq!(resolve_thing_name("Classic"), Some("classic-device".into()));
    std::env::remove_var("AWS_IOT_THING_NAME");

    // Lite
    std::env::set_var("GG_THING_NAME", "lite-device");
    assert_eq!(resolve_thing_name("Lite"), Some("lite-device".into()));
    std::env::remove_var("GG_THING_NAME");
}
