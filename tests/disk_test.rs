// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for disk space management

use gg_log_manager::disk::{enforce_disk_limit, parse_limit_bytes};
use gg_log_manager::scanner::ScannedFile;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::time::SystemTime;
use tempfile::TempDir;

fn create_file_with_size(dir: &std::path::Path, name: &str, size: usize) -> ScannedFile {
    let path = dir.join(name);
    let mut file = File::create(&path).unwrap();
    file.write_all(&vec![b'x'; size]).unwrap();
    ScannedFile {
        path,
        mtime: SystemTime::now(),
        content_hash: format!("hash_{}", name),
        is_active: false,
    }
}

fn set_mtime_offset(file: &mut ScannedFile, offset_secs: i64) {
    use std::time::{Duration, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let target = if offset_secs < 0 {
        now - Duration::from_secs((-offset_secs) as u64)
    } else {
        now + Duration::from_secs(offset_secs as u64)
    };
    file.mtime = UNIX_EPOCH + target;
    let mtime = filetime::FileTime::from_unix_time(target.as_secs() as i64, 0);
    filetime::set_file_mtime(&file.path, mtime).unwrap();
}

#[test]
fn test_disk_limit_1kb_deletes_uploaded_files() {
    let dir = TempDir::new().unwrap();

    // Configure diskSpaceLimit: "1" with diskSpaceLimitUnit: "KB" = 1024 bytes
    let limit_bytes = parse_limit_bytes("1", "KB").unwrap();
    assert_eq!(limit_bytes, 1024);

    // Write 5 files of 500 bytes each (total 2.5 KB, exceeding 1 KB limit)
    let mut file1 = create_file_with_size(dir.path(), "file1.emf.json", 500);
    let mut file2 = create_file_with_size(dir.path(), "file2.emf.json", 500);
    let mut file3 = create_file_with_size(dir.path(), "file3.emf.json", 500);
    let mut file4 = create_file_with_size(dir.path(), "file4.emf.json", 500);
    let mut file5 = create_file_with_size(dir.path(), "file5.emf.json", 500);

    // Set mtimes: file1 oldest, file5 newest (active)
    set_mtime_offset(&mut file1, -400);
    set_mtime_offset(&mut file2, -300);
    set_mtime_offset(&mut file3, -200);
    set_mtime_offset(&mut file4, -100);
    // file5 is newest - mark as active
    file5.is_active = true;

    // Mark first 3 files as uploaded
    let mut uploaded = HashSet::new();
    uploaded.insert(file1.content_hash.clone());
    uploaded.insert(file2.content_hash.clone());
    uploaded.insert(file3.content_hash.clone());

    // Files must be sorted by mtime ascending for enforce_disk_limit
    let files = vec![
        file1.clone(),
        file2.clone(),
        file3.clone(),
        file4.clone(),
        file5.clone(),
    ];

    let deleted = enforce_disk_limit(&files, limit_bytes, &uploaded, false);

    // Should delete the 3 uploaded files (oldest first) to get under limit
    // Total before: 2500 bytes, limit: 1024 bytes, need to delete ~1476 bytes
    // Deleting file1 (500) + file2 (500) + file3 (500) = 1500 bytes deleted
    assert_eq!(deleted.len(), 3, "Should delete 3 uploaded files");

    // Verify the correct files were deleted
    assert!(deleted.contains(&file1.path), "file1 should be deleted");
    assert!(deleted.contains(&file2.path), "file2 should be deleted");
    assert!(deleted.contains(&file3.path), "file3 should be deleted");

    // Verify unuploaded files are kept
    assert!(
        !deleted.contains(&file4.path),
        "file4 (unuploaded) should be kept"
    );
    assert!(
        !deleted.contains(&file5.path),
        "file5 (active) should be kept"
    );

    // Verify files actually deleted from disk
    assert!(!file1.path.exists());
    assert!(!file2.path.exists());
    assert!(!file3.path.exists());
    assert!(file4.path.exists());
    assert!(file5.path.exists());
}

#[test]
fn test_active_file_never_deleted_even_over_limit() {
    let dir = TempDir::new().unwrap();

    // Create a single large active file that exceeds the limit
    let mut active_file = create_file_with_size(dir.path(), "active.emf.json", 2000);
    active_file.is_active = true;

    // Mark it as uploaded
    let mut uploaded = HashSet::new();
    uploaded.insert(active_file.content_hash.clone());

    let files = vec![active_file.clone()];

    // Limit is 1KB (1024 bytes), file is 2000 bytes
    let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);

    // Active file should never be deleted
    assert!(deleted.is_empty(), "Active file should never be deleted");
    assert!(
        active_file.path.exists(),
        "Active file should still exist on disk"
    );
}

#[test]
fn test_uploaded_files_deleted_before_unuploaded() {
    let dir = TempDir::new().unwrap();

    // Create files: 2 uploaded (older), 2 unuploaded (newer), 1 active
    let mut uploaded1 = create_file_with_size(dir.path(), "uploaded1.emf.json", 400);
    let mut uploaded2 = create_file_with_size(dir.path(), "uploaded2.emf.json", 400);
    let mut unuploaded1 = create_file_with_size(dir.path(), "unuploaded1.emf.json", 400);
    let mut unuploaded2 = create_file_with_size(dir.path(), "unuploaded2.emf.json", 400);
    let mut active = create_file_with_size(dir.path(), "active.emf.json", 400);

    set_mtime_offset(&mut uploaded1, -500);
    set_mtime_offset(&mut uploaded2, -400);
    set_mtime_offset(&mut unuploaded1, -300);
    set_mtime_offset(&mut unuploaded2, -200);
    active.is_active = true;

    let mut uploaded_hashes = HashSet::new();
    uploaded_hashes.insert(uploaded1.content_hash.clone());
    uploaded_hashes.insert(uploaded2.content_hash.clone());

    let files = vec![
        uploaded1.clone(),
        uploaded2.clone(),
        unuploaded1.clone(),
        unuploaded2.clone(),
        active.clone(),
    ];

    // Total: 2000 bytes, limit: 1200 bytes - need to delete 800 bytes
    let deleted = enforce_disk_limit(&files, 1200, &uploaded_hashes, false);

    // Should delete uploaded files first (oldest first)
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&uploaded1.path));
    assert!(deleted.contains(&uploaded2.path));

    // Unuploaded files should be preserved
    assert!(!deleted.contains(&unuploaded1.path));
    assert!(!deleted.contains(&unuploaded2.path));
    assert!(!deleted.contains(&active.path));
}

#[test]
fn test_under_limit_no_deletion() {
    let dir = TempDir::new().unwrap();

    let file1 = create_file_with_size(dir.path(), "file1.emf.json", 100);
    let file2 = create_file_with_size(dir.path(), "file2.emf.json", 100);

    let mut uploaded = HashSet::new();
    uploaded.insert(file1.content_hash.clone());

    let files = vec![file1.clone(), file2.clone()];

    // Total: 200 bytes, limit: 1024 bytes - under limit
    let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);

    assert!(deleted.is_empty());
    assert!(file1.path.exists());
    assert!(file2.path.exists());
}

#[test]
fn test_delete_after_upload_flag() {
    let dir = TempDir::new().unwrap();

    let mut file1 = create_file_with_size(dir.path(), "file1.emf.json", 100);
    let mut file2 = create_file_with_size(dir.path(), "file2.emf.json", 100);

    set_mtime_offset(&mut file1, -100);
    file2.is_active = true;

    let mut uploaded = HashSet::new();
    uploaded.insert(file1.content_hash.clone());

    let files = vec![file1.clone(), file2.clone()];

    // Under limit, but delete_after_upload is true
    let deleted = enforce_disk_limit(&files, 10000, &uploaded, true);

    // file1 should be deleted because it's uploaded and not active
    assert_eq!(deleted.len(), 1);
    assert!(deleted.contains(&file1.path));
    assert!(!file1.path.exists());

    // file2 (active) should be kept
    assert!(file2.path.exists());
}

#[test]
fn test_parse_limit_bytes_units() {
    assert_eq!(parse_limit_bytes("1", "KB").unwrap(), 1024);
    assert_eq!(parse_limit_bytes("1", "MB").unwrap(), 1024 * 1024);
    assert_eq!(parse_limit_bytes("1", "GB").unwrap(), 1024 * 1024 * 1024);
    assert_eq!(parse_limit_bytes("100", "KB").unwrap(), 102400);

    assert!(parse_limit_bytes("abc", "KB").is_err());
    assert!(parse_limit_bytes("1", "TB").is_err());
}

#[test]
fn test_oldest_files_deleted_first() {
    let dir = TempDir::new().unwrap();

    // Create 4 uploaded files with different ages
    let mut oldest = create_file_with_size(dir.path(), "oldest.emf.json", 300);
    let mut old = create_file_with_size(dir.path(), "old.emf.json", 300);
    let mut recent = create_file_with_size(dir.path(), "recent.emf.json", 300);
    let mut active = create_file_with_size(dir.path(), "active.emf.json", 300);

    set_mtime_offset(&mut oldest, -400);
    set_mtime_offset(&mut old, -300);
    set_mtime_offset(&mut recent, -200);
    active.is_active = true;

    let mut uploaded = HashSet::new();
    uploaded.insert(oldest.content_hash.clone());
    uploaded.insert(old.content_hash.clone());
    uploaded.insert(recent.content_hash.clone());

    let files = vec![oldest.clone(), old.clone(), recent.clone(), active.clone()];

    // Total: 1200 bytes, limit: 700 bytes - need to delete 500 bytes
    let deleted = enforce_disk_limit(&files, 700, &uploaded, false);

    // Should delete oldest files first
    assert_eq!(deleted.len(), 2);
    assert!(
        deleted.contains(&oldest.path),
        "Oldest should be deleted first"
    );
    assert!(
        deleted.contains(&old.path),
        "Second oldest should be deleted"
    );
    assert!(!deleted.contains(&recent.path), "Recent should be kept");
}

#[test]
fn test_over_limit_deletes_unuploaded_when_no_uploaded() {
    let dir = TempDir::new().unwrap();
    // 3 files, 500 bytes each = 1500 total, limit 800
    let mut f1 = create_file_with_size(dir.path(), "old.json", 500);
    let mut f2 = create_file_with_size(dir.path(), "mid.json", 500);
    let mut f3 = create_file_with_size(dir.path(), "new.json", 500);
    f3.is_active = true;

    let files = vec![f1, f2, f3];
    let uploaded = HashSet::new(); // nothing uploaded
    let deleted = enforce_disk_limit(&files, 800, &uploaded, false);

    // Should delete oldest unuploaded files to get under limit
    // Need to delete ~700 bytes, so both old and mid get deleted
    assert!(deleted.len() >= 1);
    // Active file should never be deleted
    assert!(!deleted.iter().any(|p| p.ends_with("new.json")));
}

#[test]
fn test_externally_deleted_file_no_error() {
    let dir = TempDir::new().unwrap();
    let file1 = create_file_with_size(dir.path(), "file1.emf.json", 500);
    let file2 = create_file_with_size(dir.path(), "file2.emf.json", 500);
    let mut file3 = create_file_with_size(dir.path(), "file3.emf.json", 500);
    file3.is_active = true;

    let mut uploaded = HashSet::new();
    uploaded.insert(file1.content_hash.clone());

    // Externally delete the file before enforce_disk_limit runs
    std::fs::remove_file(&file1.path).unwrap();

    let files = vec![file1.clone(), file2, file3];
    let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);
    // Should not contain the externally deleted file
    assert!(!deleted.contains(&file1.path));
}

#[test]
fn test_rotated_file_not_deleted_if_hash_changed() {
    let dir = TempDir::new().unwrap();
    let mut file1 = create_file_with_size(dir.path(), "file1.emf.json", 500);
    let file2 = create_file_with_size(dir.path(), "file2.emf.json", 500);
    let mut file3 = create_file_with_size(dir.path(), "file3.emf.json", 500);
    file3.is_active = true;

    let mut uploaded = HashSet::new();
    uploaded.insert(file1.content_hash.clone());

    // Simulate file rotation: overwrite with different content
    std::fs::write(&file1.path, "completely different content after rotation").unwrap();

    let files = vec![file1.clone(), file2, file3];
    let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);
    // Rotated file should NOT be deleted because hash changed
    assert!(!deleted.contains(&file1.path));
    assert!(file1.path.exists());
}

#[test]
fn test_zero_limit_disables_disk_management() {
    let dir = TempDir::new().unwrap();
    let file1 = create_file_with_size(dir.path(), "file1.emf.json", 500);

    let mut uploaded = HashSet::new();
    uploaded.insert(file1.content_hash.clone());

    let files = vec![file1.clone()];
    // Zero limit should disable all disk management
    let deleted = enforce_disk_limit(&files, 0, &uploaded, true);
    assert!(deleted.is_empty());
    assert!(file1.path.exists());
}

#[test]
fn test_config_removal_clears_uploaded_set() {
    let dir = TempDir::new().unwrap();
    let file1 = create_file_with_size(dir.path(), "file1.emf.json", 100);

    let files = vec![file1.clone()];
    // Empty uploaded set simulates config removal — should not crash
    let deleted = enforce_disk_limit(&files, 1024, &HashSet::new(), false);
    assert!(deleted.is_empty());
    assert!(file1.path.exists());
}