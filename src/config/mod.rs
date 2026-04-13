// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Configuration management for gg-log-manager

mod schema;

pub use schema::LogManagerConfig;

use regex::Regex;
use std::fs;
use std::path::Path;

/// Load config from CLI arg - inline JSON or file path
pub fn load_config(config_arg: &str) -> Result<LogManagerConfig, String> {
    let json = if config_arg.trim_start().starts_with('{') {
        config_arg.to_string()
    } else {
        fs::read_to_string(config_arg)
            .map_err(|e| format!("Failed to read config file '{}': {}", config_arg, e))?
    };
    let config = parse_config(&json)?;
    tracing::info!("Configuration loaded successfully");
    Ok(config)
}

pub fn parse_config(json: &str) -> Result<LogManagerConfig, String> {
    serde_json::from_str(json).map_err(|e| format!("JSON parse error: {e}"))
}

pub fn validate_config(config: &LogManagerConfig) -> Result<(), String> {
    for c in &config.component_logs_configuration {
        tracing::debug!(component = %c.component_name, dir = %c.source.log_file_directory_path, "Validating component config");
        validate_directory(&c.source.log_file_directory_path)?;
        validate_regex(&c.source.log_file_regex)?;
        validate_disk_limit(&c.source.disk_space_limit, &c.source.disk_space_limit_unit)?;
    }
    for s in &config.system_logs_configuration {
        tracing::debug!(dir = %s.source.log_file_directory_path, "Validating system log config");
        validate_directory(&s.source.log_file_directory_path)?;
        validate_regex(&s.source.log_file_regex)?;
        validate_disk_limit(&s.source.disk_space_limit, &s.source.disk_space_limit_unit)?;
    }
    tracing::info!("Configuration validation passed");
    Ok(())
}

fn validate_directory(path: &str) -> Result<(), String> {
    if !Path::new(path).is_dir() {
        return Err(format!("Directory does not exist: {path}"));
    }
    Ok(())
}

fn validate_regex(pattern: &str) -> Result<(), String> {
    Regex::new(pattern).map_err(|e| format!("Invalid regex '{pattern}': {e}"))?;
    Ok(())
}

fn validate_disk_limit(limit: &str, unit: &str) -> Result<(), String> {
    let parsed: u64 = limit
        .parse()
        .map_err(|_| format!("diskSpaceLimit must be a positive number, got: {limit}"))?;
    if parsed == 0 {
        return Err("diskSpaceLimit must be positive".to_string());
    }
    if !matches!(unit, "KB" | "MB" | "GB") {
        return Err(format!("Invalid diskSpaceLimitUnit: {unit}"));
    }
    Ok(())
}

/// Parse diskSpaceLimit string to u64
pub fn parse_disk_space_limit(limit: &str) -> Result<u64, String> {
    limit
        .parse()
        .map_err(|_| format!("Invalid diskSpaceLimit: {limit}"))
}

/// Derive log group name: /aws/greengrass/{componentType}/{region}/{componentName}
pub fn derive_log_group_name(component_name: &str, component_type: Option<&str>) -> String {
    let comp_type = component_type.unwrap_or("UserComponent");
    let region = std::env::var("AWS_DEFAULT_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    format!("/aws/greengrass/{comp_type}/{region}/{component_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn test_parse_config_empty() {
        let config = parse_config("{}").unwrap();
        assert!(config.component_logs_configuration.is_empty());
        assert!(config.system_logs_configuration.is_empty());
        assert_eq!(config.periodic_upload_interval_sec, 300);
    }

    #[test]
    fn test_validate_directory_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_directory(dir.path().to_str().unwrap()).is_ok());
    }

    #[test]
    fn test_validate_directory_not_exists() {
        let result = validate_directory("/nonexistent/path/12345");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_validate_regex_valid() {
        assert!(validate_regex(r".*\.log$").is_ok());
        assert!(validate_regex(r"^\d{4}-\d{2}-\d{2}").is_ok());
    }

    #[test]
    fn test_validate_regex_invalid() {
        let result = validate_regex(r"[invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_disk_limit_valid() {
        assert!(validate_disk_limit("100", "KB").is_ok());
        assert!(validate_disk_limit("25", "MB").is_ok());
        assert!(validate_disk_limit("1", "GB").is_ok());
    }

    #[test]
    fn test_validate_disk_limit_zero() {
        let result = validate_disk_limit("0", "MB");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("positive"));
    }

    #[test]
    fn test_validate_disk_limit_invalid_unit() {
        let result = validate_disk_limit("100", "TB");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid"));
    }

    #[test]
    fn test_validate_disk_limit_non_numeric() {
        let result = validate_disk_limit("abc", "MB");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_config_valid() {
        let dir = tempfile::tempdir().unwrap();
        let json = format!(
            r#"{{
                "componentLogsConfiguration": [{{
                    "componentName": "test",
                    "logFileDirectoryPath": "{}",
                    "logFileRegex": ".*\\.log$"
                }}]
            }}"#,
            dir.path().to_str().unwrap()
        );
        let config = parse_config(&json).unwrap();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_config_system_logs() {
        let dir = tempfile::tempdir().unwrap();
        let json = format!(
            r#"{{
                "systemLogsConfiguration": [{{
                    "logFileDirectoryPath": "{}",
                    "logFileRegex": ".*\\.log$",
                    "logGroupName": "/aws/greengrass/system"
                }}]
            }}"#,
            dir.path().to_str().unwrap()
        );
        let config = parse_config(&json).unwrap();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    #[serial]
    fn test_derive_log_group_name_with_region() {
        std::env::set_var("AWS_DEFAULT_REGION", "eu-west-1");
        let name = derive_log_group_name("my-component", Some("GreengrassSystemComponent"));
        assert_eq!(
            name,
            "/aws/greengrass/GreengrassSystemComponent/eu-west-1/my-component"
        );
        std::env::remove_var("AWS_DEFAULT_REGION");
    }

    #[test]
    #[serial]
    fn test_derive_log_group_name_default_region() {
        std::env::remove_var("AWS_DEFAULT_REGION");
        let name = derive_log_group_name("my-component", None);
        assert_eq!(name, "/aws/greengrass/UserComponent/us-east-1/my-component");
    }

    #[test]
    fn test_parse_disk_space_limit() {
        assert_eq!(parse_disk_space_limit("100").unwrap(), 100);
        assert_eq!(parse_disk_space_limit("0").unwrap(), 0);
        assert!(parse_disk_space_limit("abc").is_err());
    }

    #[test]
    fn test_load_config_inline_json() {
        let config = load_config("{}").unwrap();
        assert!(config.component_logs_configuration.is_empty());
    }

    #[test]
    fn test_load_config_file_not_found() {
        let result = load_config("/nonexistent/config.json");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to read"));
    }

    const FULL_CONFIG_JSON: &str = r#"{
        "periodicUploadIntervalSec": 300,
        "componentLogsConfiguration": [
            {
                "componentName": "com.example.MyApp",
                "logFileDirectoryPath": "/tmp",
                "logFileRegex": "app\\.log.*",
                "diskSpaceLimit": "25",
                "diskSpaceLimitUnit": "MB",
                "deleteLogFileAfterCloudUpload": false
            },
            {
                "componentName": "aws.greengrass.Nucleus",
                "logFileDirectoryPath": "/tmp",
                "logFileRegex": "greengrass\\.log.*",
                "logGroupName": "/custom/nucleus/logs",
                "minimumLogLevel": "DEBUG",
                "diskSpaceLimit": "100",
                "diskSpaceLimitUnit": "MB",
                "uploadIntervalSec": 60
            },
            {
                "componentName": "com.example.EMFApp",
                "logFileDirectoryPath": "/tmp",
                "logFileRegex": "emf-.*\\.json",
                "diskSpaceLimit": "10",
                "multiLineStartPattern": "^\\{"
            }
        ],
        "systemLogsConfiguration": [
            {
                "logFileDirectoryPath": "/tmp",
                "logFileRegex": "syslog.*",
                "logGroupName": "/aws/greengrass/system/syslog",
                "diskSpaceLimit": "50",
                "diskSpaceLimitUnit": "GB"
            }
        ]
    }"#;

    #[test]
    fn test_parse_config_fields() {
        let config = parse_config(FULL_CONFIG_JSON).unwrap();
        assert_eq!(config.periodic_upload_interval_sec, 300);
        assert_eq!(config.component_logs_configuration.len(), 3);
        assert_eq!(config.system_logs_configuration.len(), 1);
    }

    #[test]
    fn test_component_config_fields() {
        let config = parse_config(FULL_CONFIG_JSON).unwrap();
        let comp = &config.component_logs_configuration[0];
        assert_eq!(comp.component_name, "com.example.MyApp");
        assert_eq!(comp.source.log_file_directory_path, "/tmp");
        assert_eq!(comp.source.disk_space_limit, "25");
        assert_eq!(comp.source.disk_space_limit_unit, "MB");
        assert!(!comp.source.delete_log_file_after_cloud_upload);
    }

    #[test]
    fn test_upload_interval_override() {
        let config = parse_config(FULL_CONFIG_JSON).unwrap();
        let nucleus = &config.component_logs_configuration[1];
        assert_eq!(nucleus.source.upload_interval_sec, Some(60));
        assert_eq!(nucleus.log_group_name, Some("/custom/nucleus/logs".to_string()));
        assert_eq!(nucleus.source.minimum_log_level, "DEBUG");
    }

    #[test]
    fn test_defaults_applied() {
        let config = parse_config(FULL_CONFIG_JSON).unwrap();
        let emf = &config.component_logs_configuration[2];
        assert_eq!(emf.source.minimum_log_level, "INFO");
        assert_eq!(emf.source.disk_space_limit_unit, "MB");
        assert!(!emf.source.delete_log_file_after_cloud_upload);
    }

    #[test]
    fn test_system_config_fields() {
        let config = parse_config(FULL_CONFIG_JSON).unwrap();
        let sys = &config.system_logs_configuration[0];
        assert_eq!(sys.log_group_name, "/aws/greengrass/system/syslog");
        assert_eq!(sys.source.disk_space_limit, "50");
        assert_eq!(sys.source.disk_space_limit_unit, "GB");
    }

    #[test]
    fn test_default_periodic_interval() {
        let config = parse_config("{}").unwrap();
        assert_eq!(config.periodic_upload_interval_sec, 300);
    }

    #[test]
    fn test_disk_space_limit_string_parsing() {
        assert_eq!(parse_disk_space_limit("100").unwrap(), 100);
        assert_eq!(parse_disk_space_limit("0").unwrap(), 0);
        assert!(parse_disk_space_limit("abc").is_err());
        assert!(parse_disk_space_limit("-5").is_err());
    }

    #[test]
    fn test_validate_config_invalid_disk_limit() {
        let config = parse_config(r#"{"componentLogsConfiguration":[{"componentName":"t","logFileDirectoryPath":"/tmp","logFileRegex":".*","diskSpaceLimit":"abc"}]}"#).unwrap();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_invalid_unit() {
        let config = parse_config(r#"{"componentLogsConfiguration":[{"componentName":"t","logFileDirectoryPath":"/tmp","logFileRegex":".*","diskSpaceLimit":"100","diskSpaceLimitUnit":"TB"}]}"#).unwrap();
        assert!(validate_config(&config).unwrap_err().contains("Invalid diskSpaceLimitUnit"));
    }

    #[test]
    fn test_validate_config_invalid_regex() {
        let config = parse_config(r#"{"componentLogsConfiguration":[{"componentName":"t","logFileDirectoryPath":"/tmp","logFileRegex":"[invalid","diskSpaceLimit":"100"}]}"#).unwrap();
        assert!(validate_config(&config).unwrap_err().contains("Invalid regex"));
    }

    #[test]
    fn test_validate_config_zero_disk_limit() {
        let config = parse_config(r#"{"componentLogsConfiguration":[{"componentName":"t","logFileDirectoryPath":"/tmp","logFileRegex":".*","diskSpaceLimit":"0"}]}"#).unwrap();
        assert!(validate_config(&config).unwrap_err().contains("must be positive"));
    }
}
