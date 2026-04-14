// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileCheckpoint {
    pub content_hash: String,
    pub start_position: u64,
    pub last_modified_time: u64,
    pub last_accessed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LastFileProcessedTimestamp {
    pub last_file_processed_time_stamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointStore {
    #[serde(default)]
    pub current_component_file_processing_information_v2:
        HashMap<String, HashMap<String, FileCheckpoint>>,
    #[serde(default)]
    pub component_last_file_processed_time_stamp: HashMap<String, LastFileProcessedTimestamp>,
}

pub fn save_checkpoint(path: &Path, store: &CheckpointStore) -> io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut file = fs::File::create(&tmp_path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp_path, path)?;
    tracing::debug!(path = %path.display(), "Checkpoint saved");
    Ok(())
}

pub fn load_checkpoint(path: &Path) -> io::Result<CheckpointStore> {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<CheckpointStore>(&content) {
            Ok(store) => {
                let entry_count: usize = store
                    .current_component_file_processing_information_v2
                    .values()
                    .map(|m| m.len())
                    .sum();
                tracing::debug!(path = %path.display(), entry_count = entry_count, "Checkpoint loaded");
                Ok(store)
            }
            Err(e) => {
                tracing::warn!("Corrupt checkpoint file, starting fresh: {}", e);
                Ok(CheckpointStore::default())
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(CheckpointStore::default()),
        Err(e) => Err(e),
    }
}

use super::ScannedFile;
use std::collections::HashSet;
use std::path::PathBuf;

/// Recover file offsets from checkpoint for restart recovery.
/// Returns (file_path, resume_offset) pairs for each scanned file.
/// Removes stale checkpoint entries for files no longer present.
pub fn recover_offsets(
    checkpoint: &mut CheckpointStore,
    log_group_key: &str,
    scanned_files: &[ScannedFile],
) -> Vec<(PathBuf, u64)> {
    let current_hashes: HashSet<&str> = scanned_files
        .iter()
        .map(|f| f.content_hash.as_str())
        .collect();

    let result: Vec<(PathBuf, u64)> = scanned_files
        .iter()
        .map(|file| {
            let offset = checkpoint
                .current_component_file_processing_information_v2
                .get(log_group_key)
                .and_then(|m| m.get(&file.content_hash))
                .map(|cp| {
                    tracing::info!(
                        "Resuming file {} from offset {}",
                        file.path.display(),
                        cp.start_position
                    );
                    cp.start_position
                })
                .unwrap_or_else(|| {
                    tracing::info!("New file {}, reading from start", file.path.display());
                    0
                });
            (file.path.clone(), offset)
        })
        .collect();

    // Remove stale entries
    if let Some(file_map) = checkpoint
        .current_component_file_processing_information_v2
        .get_mut(log_group_key)
    {
        file_map.retain(|hash, _| {
            let keep = current_hashes.contains(hash.as_str());
            if !keep {
                tracing::info!("Removing stale checkpoint for hash {}", hash);
            }
            keep
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_save_and_reload() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("checkpoint.json");

        let mut store = CheckpointStore::default();
        let mut files = HashMap::new();
        files.insert(
            "abc123".to_string(),
            FileCheckpoint {
                content_hash: "abc123".to_string(),
                start_position: 1024,
                last_modified_time: 1700000000000,
                last_accessed: 1700000001000,
            },
        );
        store
            .current_component_file_processing_information_v2
            .insert("/aws/greengrass/test".to_string(), files);
        store.component_last_file_processed_time_stamp.insert(
            "/aws/greengrass/test".to_string(),
            LastFileProcessedTimestamp {
                last_file_processed_time_stamp: 1700000000000,
            },
        );

        save_checkpoint(&path, &store).unwrap();
        let loaded = load_checkpoint(&path).unwrap();

        assert_eq!(store, loaded);
    }

    #[test]
    fn test_atomic_write_no_partial() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("checkpoint.json");
        let tmp_path = path.with_extension("tmp");

        let store = CheckpointStore::default();
        save_checkpoint(&path, &store).unwrap();

        // After successful save, tmp file should not exist
        assert!(!tmp_path.exists());
        assert!(path.exists());
    }

    #[test]
    fn test_missing_file_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");

        let store = load_checkpoint(&path).unwrap();

        assert!(store
            .current_component_file_processing_information_v2
            .is_empty());
        assert!(store.component_last_file_processed_time_stamp.is_empty());
    }

    #[test]
    fn test_corrupt_json_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        fs::write(&path, "{ invalid json }").unwrap();

        let result = load_checkpoint(&path).unwrap();
        assert!(result
            .current_component_file_processing_information_v2
            .is_empty());
    }

    #[test]
    fn test_json_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("checkpoint.json");

        let mut store = CheckpointStore::default();
        let mut files = HashMap::new();
        files.insert(
            "deadbeef".to_string(),
            FileCheckpoint {
                content_hash: "deadbeef".to_string(),
                start_position: 512,
                last_modified_time: 1600000000000,
                last_accessed: 1600000001000,
            },
        );
        store
            .current_component_file_processing_information_v2
            .insert("logGroup1".to_string(), files);

        save_checkpoint(&path, &store).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("currentComponentFileProcessingInformationV2"));
        assert!(content.contains("contentHash"));
        assert!(content.contains("startPosition"));
        assert!(content.contains("lastModifiedTime"));
        assert!(content.contains("lastAccessed"));
    }

    #[test]
    fn test_recover_offsets_matching_hash() {
        use super::recover_offsets;
        use crate::scanner::ScannedFile;
        use std::time::SystemTime;

        let mut store = CheckpointStore::default();
        let mut files = HashMap::new();
        files.insert(
            "hash123".to_string(),
            FileCheckpoint {
                content_hash: "hash123".to_string(),
                start_position: 1024,
                last_modified_time: 1700000000000,
                last_accessed: 1700000001000,
            },
        );
        store
            .current_component_file_processing_information_v2
            .insert("test-group".to_string(), files);

        let scanned = vec![ScannedFile {
            path: PathBuf::from("/var/log/app.log"),
            mtime: SystemTime::now(),
            content_hash: "hash123".to_string(),
            is_active: true,
        }];

        let result = recover_offsets(&mut store, "test-group", &scanned);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, PathBuf::from("/var/log/app.log"));
        assert_eq!(result[0].1, 1024);
    }

    #[test]
    fn test_recover_offsets_new_file() {
        use super::recover_offsets;
        use crate::scanner::ScannedFile;
        use std::time::SystemTime;

        let mut store = CheckpointStore::default();

        let scanned = vec![ScannedFile {
            path: PathBuf::from("/var/log/new.log"),
            mtime: SystemTime::now(),
            content_hash: "newhash".to_string(),
            is_active: true,
        }];

        let result = recover_offsets(&mut store, "test-group", &scanned);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, PathBuf::from("/var/log/new.log"));
        assert_eq!(result[0].1, 0);
    }

    #[test]
    fn test_recover_offsets_removes_stale() {
        use super::recover_offsets;
        use crate::scanner::ScannedFile;
        use std::time::SystemTime;

        let mut store = CheckpointStore::default();
        let mut files = HashMap::new();
        files.insert(
            "stale_hash".to_string(),
            FileCheckpoint {
                content_hash: "stale_hash".to_string(),
                start_position: 500,
                last_modified_time: 1700000000000,
                last_accessed: 1700000001000,
            },
        );
        files.insert(
            "current_hash".to_string(),
            FileCheckpoint {
                content_hash: "current_hash".to_string(),
                start_position: 200,
                last_modified_time: 1700000000000,
                last_accessed: 1700000001000,
            },
        );
        store
            .current_component_file_processing_information_v2
            .insert("test-group".to_string(), files);

        let scanned = vec![ScannedFile {
            path: PathBuf::from("/var/log/current.log"),
            mtime: SystemTime::now(),
            content_hash: "current_hash".to_string(),
            is_active: true,
        }];

        let _ = recover_offsets(&mut store, "test-group", &scanned);

        let remaining = store
            .current_component_file_processing_information_v2
            .get("test-group")
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining.contains_key("current_hash"));
        assert!(!remaining.contains_key("stale_hash"));
    }
}
