// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for checkpoint persistence and restart recovery

use gg_log_manager::scanner::{
    compute_content_hash, load_checkpoint, recover_offsets, save_checkpoint, CheckpointStore,
    FileCheckpoint, ScannedFile,
};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;
use tempfile::TempDir;

fn create_emf_file(dir: &std::path::Path, name: &str, content: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let mut file = File::create(&path).unwrap();
    file.write_all(content).unwrap();
    path
}

#[test]
fn test_checkpoint_persistence_and_reload() {
    let dir = TempDir::new().unwrap();
    let checkpoint_path = dir.path().join("checkpoint.json");

    // Create 3 EMF files
    let file1 = create_emf_file(dir.path(), "file1.emf.json", b"content1");
    let file2 = create_emf_file(dir.path(), "file2.emf.json", b"content2");
    let file3 = create_emf_file(dir.path(), "file3.emf.json", b"content3");

    // Compute content hashes
    let hash1 = compute_content_hash(&file1).unwrap();
    let hash2 = compute_content_hash(&file2).unwrap();
    let hash3 = compute_content_hash(&file3).unwrap();

    // Create checkpoint entries
    let mut store = CheckpointStore::default();
    let mut files_map = HashMap::new();

    files_map.insert(
        hash1.clone(),
        FileCheckpoint {
            content_hash: hash1.clone(),
            start_position: 100,
            last_modified_time: 1700000000000,
            last_accessed: 1700000001000,
        },
    );
    files_map.insert(
        hash2.clone(),
        FileCheckpoint {
            content_hash: hash2.clone(),
            start_position: 200,
            last_modified_time: 1700000002000,
            last_accessed: 1700000003000,
        },
    );
    files_map.insert(
        hash3.clone(),
        FileCheckpoint {
            content_hash: hash3.clone(),
            start_position: 300,
            last_modified_time: 1700000004000,
            last_accessed: 1700000005000,
        },
    );

    store
        .current_component_file_processing_information_v2
        .insert("test-log-group".to_string(), files_map);

    // Save checkpoint to disk
    save_checkpoint(&checkpoint_path, &store).unwrap();

    // Load checkpoint from disk
    let loaded = load_checkpoint(&checkpoint_path).unwrap();

    // Verify entries match
    assert_eq!(store, loaded);

    let loaded_files = loaded
        .current_component_file_processing_information_v2
        .get("test-log-group")
        .unwrap();

    assert_eq!(loaded_files.len(), 3);
    assert_eq!(loaded_files.get(&hash1).unwrap().start_position, 100);
    assert_eq!(loaded_files.get(&hash2).unwrap().start_position, 200);
    assert_eq!(loaded_files.get(&hash3).unwrap().start_position, 300);
}

#[test]
fn test_checkpoint_recovery_with_file_deletion() {
    let dir = TempDir::new().unwrap();

    // Create 3 EMF files
    let file1 = create_emf_file(dir.path(), "file1.emf.json", b"content1");
    let file2 = create_emf_file(dir.path(), "file2.emf.json", b"content2");
    let file3 = create_emf_file(dir.path(), "file3.emf.json", b"content3");

    // Compute content hashes
    let hash1 = compute_content_hash(&file1).unwrap();
    let hash2 = compute_content_hash(&file2).unwrap();
    let hash3 = compute_content_hash(&file3).unwrap();

    // Create checkpoint with entries for all 3 files
    let mut store = CheckpointStore::default();
    let mut files_map = HashMap::new();

    files_map.insert(
        hash1.clone(),
        FileCheckpoint {
            content_hash: hash1.clone(),
            start_position: 100,
            last_modified_time: 1700000000000,
            last_accessed: 1700000001000,
        },
    );
    files_map.insert(
        hash2.clone(),
        FileCheckpoint {
            content_hash: hash2.clone(),
            start_position: 200,
            last_modified_time: 1700000002000,
            last_accessed: 1700000003000,
        },
    );
    files_map.insert(
        hash3.clone(),
        FileCheckpoint {
            content_hash: hash3.clone(),
            start_position: 300,
            last_modified_time: 1700000004000,
            last_accessed: 1700000005000,
        },
    );

    store
        .current_component_file_processing_information_v2
        .insert("test-log-group".to_string(), files_map);

    // Simulate file deletion - remove file2
    fs::remove_file(&file2).unwrap();

    // Create scanned files list (only file1 and file3 remain)
    let scanned_files = vec![
        ScannedFile {
            path: file1.clone(),
            mtime: SystemTime::now(),
            content_hash: hash1.clone(),
            is_active: false,
        },
        ScannedFile {
            path: file3.clone(),
            mtime: SystemTime::now(),
            content_hash: hash3.clone(),
            is_active: true,
        },
    ];

    // Run recover_offsets
    let offsets = recover_offsets(&mut store, "test-log-group", &scanned_files);

    // Verify offsets returned for existing files
    assert_eq!(offsets.len(), 2);

    let offset1 = offsets.iter().find(|(p, _)| *p == file1).unwrap();
    assert_eq!(offset1.1, 100);

    let offset3 = offsets.iter().find(|(p, _)| *p == file3).unwrap();
    assert_eq!(offset3.1, 300);

    // Verify stale entry (file2) is removed from checkpoint
    let remaining = store
        .current_component_file_processing_information_v2
        .get("test-log-group")
        .unwrap();

    assert_eq!(remaining.len(), 2);
    assert!(remaining.contains_key(&hash1));
    assert!(
        !remaining.contains_key(&hash2),
        "Stale entry should be removed"
    );
    assert!(remaining.contains_key(&hash3));
}

#[test]
fn test_checkpoint_new_file_returns_zero_offset() {
    let dir = TempDir::new().unwrap();

    // Create a new file not in checkpoint
    let new_file = create_emf_file(dir.path(), "new.emf.json", b"new content");
    let new_hash = compute_content_hash(&new_file).unwrap();

    // Empty checkpoint
    let mut store = CheckpointStore::default();
    store
        .current_component_file_processing_information_v2
        .insert("test-log-group".to_string(), HashMap::new());

    let scanned_files = vec![ScannedFile {
        path: new_file.clone(),
        mtime: SystemTime::now(),
        content_hash: new_hash,
        is_active: true,
    }];

    let offsets = recover_offsets(&mut store, "test-log-group", &scanned_files);

    assert_eq!(offsets.len(), 1);
    assert_eq!(offsets[0].0, new_file);
    assert_eq!(offsets[0].1, 0, "New file should start from offset 0");
}

#[test]
fn test_checkpoint_missing_file_returns_empty() {
    let dir = TempDir::new().unwrap();
    let checkpoint_path = dir.path().join("nonexistent.json");

    let store = load_checkpoint(&checkpoint_path).unwrap();

    assert!(store
        .current_component_file_processing_information_v2
        .is_empty());
    assert!(store.component_last_file_processed_time_stamp.is_empty());
}

#[test]
fn test_checkpoint_atomic_write() {
    let dir = TempDir::new().unwrap();
    let checkpoint_path = dir.path().join("checkpoint.json");
    let tmp_path = checkpoint_path.with_extension("tmp");

    let store = CheckpointStore::default();
    save_checkpoint(&checkpoint_path, &store).unwrap();

    // After successful save, tmp file should not exist
    assert!(!tmp_path.exists(), "Temp file should be cleaned up");
    assert!(checkpoint_path.exists(), "Checkpoint file should exist");
}

#[test]
fn test_checkpoint_json_format() {
    let dir = TempDir::new().unwrap();
    let checkpoint_path = dir.path().join("checkpoint.json");

    let mut store = CheckpointStore::default();
    let mut files_map = HashMap::new();
    files_map.insert(
        "testhash".to_string(),
        FileCheckpoint {
            content_hash: "testhash".to_string(),
            start_position: 512,
            last_modified_time: 1600000000000,
            last_accessed: 1600000001000,
        },
    );
    store
        .current_component_file_processing_information_v2
        .insert("logGroup1".to_string(), files_map);

    save_checkpoint(&checkpoint_path, &store).unwrap();

    let content = fs::read_to_string(&checkpoint_path).unwrap();

    // Verify camelCase JSON keys
    assert!(content.contains("currentComponentFileProcessingInformationV2"));
    assert!(content.contains("contentHash"));
    assert!(content.contains("startPosition"));
    assert!(content.contains("lastModifiedTime"));
    assert!(content.contains("lastAccessed"));
}

#[test]
fn test_load_corrupt_checkpoint_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.json");
    std::fs::write(&path, "not valid json {{{").unwrap();
    let store = gg_log_manager::scanner::load_checkpoint(&path).unwrap();
    assert!(store
        .current_component_file_processing_information_v2
        .is_empty());
}

#[test]
fn test_load_checkpoint_permission_error() {
    // Non-existent returns empty (already tested), but let's test a path
    // that exists but is a directory (will fail with a different error)
    let dir = tempfile::tempdir().unwrap();
    let result = gg_log_manager::scanner::load_checkpoint(dir.path());
    // Reading a directory as a file should error
    assert!(result.is_err());
}

#[test]
fn test_load_java_format_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.tlog");
    // Write a checkpoint in Java's format
    let java_format = r#"{"currentComponentFileProcessingInformationV2":{"test-component":{"componentType":"UserComponent","logFileInformationMap":{"/var/log/test.log":{"startPosition":1024,"fileHash":"abc123"}}}}}"#;
    std::fs::write(&path, java_format).unwrap();
    let store = load_checkpoint(&path).unwrap();
    // Verify we can read the Java format
    assert!(!store.current_component_file_processing_information_v2.is_empty());
}
