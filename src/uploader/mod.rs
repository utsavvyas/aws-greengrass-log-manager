// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Upload pipeline - batching, CloudWatch client, retry logic, scheduling

mod batcher;
mod cw_client;
mod retry;

pub use batcher::{batch_events, SealedBatch};
pub use cw_client::CwLogsClient;
pub use retry::{format_log_stream_name, upload_with_retry};

use crate::scanner::{CheckpointStore, FileCheckpoint, LogEvent};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Result of uploading events for a single log source.
/// Contains the files whose checkpoints should be advanced (only on success).
pub struct UploadResult {
    /// Files that were successfully uploaded — advance their checkpoints
    pub succeeded: Vec<(PathBuf, u64)>,
    /// Files that failed upload — do NOT advance checkpoints (re-read next cycle)
    pub failed: Vec<PathBuf>,
}

/// Upload events for a single log source. Returns which files succeeded/failed.
/// On failure, checkpoints are NOT advanced — scanner re-reads same data next cycle.
#[cfg(not(tarpaulin_include))]
pub async fn upload_source_events(
    client: &mut CwLogsClient,
    log_group: &str,
    thing_name: &str,
    file_events: Vec<(PathBuf, Vec<LogEvent>, u64)>, // (path, events, new_offset)
) -> UploadResult {
    let log_stream = format_log_stream_name(thing_name);
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();

    for (path, events, new_offset) in file_events {
        if events.is_empty() {
            succeeded.push((path, new_offset));
            continue;
        }

        let batches = batch_events(log_group, &log_stream, events);
        let mut all_ok = true;

        for batch in &batches {
            match upload_with_retry(client, batch).await {
                Ok(true) => {}
                Ok(false) => {
                    // All retries exhausted — do NOT advance checkpoint (1.4.5)
                    tracing::error!(
                        "Upload failed after 5 retries for log group {}, events will be re-read next cycle",
                        log_group
                    );
                    all_ok = false;
                    break;
                }
                Err(e) => {
                    tracing::error!("Upload error for {}: {}", log_group, e);
                    all_ok = false;
                    break;
                }
            }
        }

        if all_ok {
            succeeded.push((path, new_offset));
        } else {
            failed.push(path);
        }
    }

    UploadResult { succeeded, failed }
}

/// Compute the effective upload interval for a log source.
/// Uses per-source `uploadIntervalSec` if set, otherwise global `periodicUploadIntervalSec`.
/// Adds random jitter of 0-5 seconds to prevent thundering herd.
pub fn effective_interval_secs(per_source: Option<u64>, global: u64) -> u64 {
    let base = per_source.unwrap_or(global);
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64
        % 6; // 0-5 seconds
    base + jitter
}

/// Update checkpoint store after successful uploads.
pub fn advance_checkpoints(
    store: &mut CheckpointStore,
    log_group_key: &str,
    succeeded: &[(PathBuf, u64)],
    content_hashes: &HashMap<PathBuf, String>,
) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let group_map = store
        .current_component_file_processing_information_v2
        .entry(log_group_key.to_string())
        .or_default();

    for (path, new_offset) in succeeded {
        if let Some(hash) = content_hashes.get(path) {
            group_map.insert(
                hash.clone(),
                FileCheckpoint {
                    content_hash: hash.clone(),
                    start_position: *new_offset,
                    last_modified_time: now_ms,
                    last_accessed: now_ms,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effective_interval_with_override() {
        let interval = effective_interval_secs(Some(60), 300);
        assert!((60..=65).contains(&interval));
    }

    #[test]
    fn test_effective_interval_global_default() {
        let interval = effective_interval_secs(None, 300);
        assert!((300..=305).contains(&interval));
    }

    #[test]
    fn test_effective_interval_zero_global() {
        // Edge case: global interval is 0
        let interval = effective_interval_secs(None, 0);
        assert!((0..=5).contains(&interval)); // Just jitter
    }

    #[test]
    fn test_advance_checkpoints() {
        let mut store = CheckpointStore::default();
        let mut hashes = HashMap::new();
        let path = PathBuf::from("/tmp/test.emf.json");
        hashes.insert(path.clone(), "abc123".to_string());

        advance_checkpoints(&mut store, "system-health", &[(path, 1024)], &hashes);

        let group = &store.current_component_file_processing_information_v2["system-health"];
        let cp = &group["abc123"];
        assert_eq!(cp.start_position, 1024);
        assert_eq!(cp.content_hash, "abc123");
    }

    #[test]
    fn test_advance_checkpoints_empty_succeeded() {
        let mut store = CheckpointStore::default();
        let hashes = HashMap::new();

        advance_checkpoints(&mut store, "system-health", &[], &hashes);

        // Should create the group entry but with no checkpoints
        assert!(store
            .current_component_file_processing_information_v2
            .get("system-health")
            .map_or(true, |m| m.is_empty()));
    }

    #[test]
    fn test_advance_checkpoints_missing_hash() {
        let mut store = CheckpointStore::default();
        let hashes = HashMap::new(); // Empty - no hash for the path
        let path = PathBuf::from("/tmp/test.emf.json");

        advance_checkpoints(&mut store, "system-health", &[(path, 1024)], &hashes);

        // Should not add checkpoint if hash is missing
        let group = store
            .current_component_file_processing_information_v2
            .get("system-health");
        assert!(group.map_or(true, |m| m.is_empty()));
    }

    #[test]
    fn test_upload_result_struct() {
        let result = UploadResult {
            succeeded: vec![(PathBuf::from("/a"), 100), (PathBuf::from("/b"), 200)],
            failed: vec![PathBuf::from("/c")],
        };
        assert_eq!(result.succeeded.len(), 2);
        assert_eq!(result.failed.len(), 1);
    }
}
