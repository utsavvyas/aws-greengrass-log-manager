// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Configuration schema structs matching Java LogManager JSON schema

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogManagerConfig {
    #[serde(default)]
    pub component_logs_configuration: Vec<ComponentLogConfig>,
    #[serde(default)]
    pub system_logs_configuration: Vec<SystemLogConfig>,
    #[serde(default = "default_periodic_interval")]
    pub periodic_upload_interval_sec: u64,
}

fn default_periodic_interval() -> u64 {
    300
}

/// Shared log source fields used by both component and system configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogSourceConfig {
    pub log_file_directory_path: String,
    pub log_file_regex: String,
    #[serde(default = "default_log_level")]
    pub minimum_log_level: String,
    #[serde(default = "default_disk_limit")]
    pub disk_space_limit: String,
    #[serde(default = "default_disk_unit")]
    pub disk_space_limit_unit: String,
    #[serde(default)]
    pub delete_log_file_after_cloud_upload: bool,
    pub multi_line_start_pattern: Option<String>,
    pub upload_interval_sec: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentLogConfig {
    pub component_name: String,
    pub log_group_name: Option<String>,
    #[serde(flatten)]
    pub source: LogSourceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemLogConfig {
    pub log_group_name: String,
    #[serde(flatten)]
    pub source: LogSourceConfig,
}

fn default_log_level() -> String {
    "INFO".to_string()
}

fn default_disk_unit() -> String {
    "MB".to_string()
}

fn default_disk_limit() -> String {
    "25".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let config: LogManagerConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.periodic_upload_interval_sec, 300);
        assert!(config.component_logs_configuration.is_empty());
        assert!(config.system_logs_configuration.is_empty());
    }

    #[test]
    fn test_component_defaults() {
        let json = r#"{"componentName":"test","logFileDirectoryPath":"/tmp","logFileRegex":".*"}"#;
        let comp: ComponentLogConfig = serde_json::from_str(json).unwrap();
        assert_eq!(comp.source.disk_space_limit, "25");
        assert_eq!(comp.source.disk_space_limit_unit, "MB");
        assert_eq!(comp.source.minimum_log_level, "INFO");
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = LogManagerConfig {
            periodic_upload_interval_sec: 600,
            component_logs_configuration: vec![ComponentLogConfig {
                component_name: "test".into(),
                log_group_name: Some("/test/logs".into()),
                source: LogSourceConfig {
                    log_file_directory_path: "/var/log".into(),
                    log_file_regex: r".*\.log".into(),
                    minimum_log_level: "DEBUG".into(),
                    disk_space_limit: "50".into(),
                    disk_space_limit_unit: "GB".into(),
                    delete_log_file_after_cloud_upload: true,
                    multi_line_start_pattern: Some(r"^\d".into()),
                    upload_interval_sec: Some(120),
                },
            }],
            system_logs_configuration: vec![],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: LogManagerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.periodic_upload_interval_sec, 600);
        let comp = &parsed.component_logs_configuration[0];
        assert_eq!(comp.component_name, "test");
        assert_eq!(comp.source.disk_space_limit, "50");
        assert_eq!(comp.source.upload_interval_sec, Some(120));
    }
}
