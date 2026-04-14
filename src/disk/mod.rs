// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Disk space management - per-log-group limits, file cleanup

use crate::scanner::{compute_content_hash, ScannedFile};
use std::collections::HashSet;
use std::fs;

/// Parse disk space limit string + unit to bytes.
pub fn parse_limit_bytes(limit: &str, unit: &str) -> Result<u64, String> {
    let value: u64 = limit
        .parse()
        .map_err(|_| format!("Invalid diskSpaceLimit: {limit}"))?;
    match unit {
        "KB" => Ok(value * 1024),
        "MB" => Ok(value * 1024 * 1024),
        "GB" => Ok(value * 1024 * 1024 * 1024),
        _ => Err(format!("Invalid unit: {unit}")),
    }
}

/// Enforce disk space limit for a log source directory.
/// Deletes oldest files first, preferring already-uploaded files.
/// Never deletes the active file (newest by mtime).
///
/// `uploaded_hashes`: content hashes of files fully uploaded to CW.
/// `delete_after_upload`: if true, delete uploaded non-active files regardless of limit.
///
/// Returns list of deleted file paths.
pub fn enforce_disk_limit(
    files: &[ScannedFile],
    limit_bytes: u64,
    uploaded_hashes: &HashSet<String>,
    delete_after_upload: bool,
) -> HashSet<std::path::PathBuf> {
    let mut deleted: HashSet<std::path::PathBuf> = HashSet::new();

    // Zero limit disables disk management
    if limit_bytes == 0 {
        return deleted;
    }

    // Phase 1: deleteLogFileAfterCloudUpload — delete uploaded non-active files (1.6.2)
    if delete_after_upload {
        for file in files {
            if !file.is_active && uploaded_hashes.contains(&file.content_hash) {
                if !file.path.exists() {
                    continue; // Externally deleted
                }
                if let Ok(()) = fs::remove_file(&file.path) {
                    tracing::info!("Deleted uploaded file: {}", file.path.display());
                    deleted.insert(file.path.clone());
                }
            }
        }
    }

    // Phase 2: Check total size against limit (1.6.1)
    let remaining: Vec<&ScannedFile> = files
        .iter()
        .filter(|f| !deleted.contains(&f.path))
        .collect();

    let total_bytes: u64 = remaining
        .iter()
        .filter_map(|f| fs::metadata(&f.path).ok())
        .map(|m| m.len())
        .sum();

    tracing::debug!(
        total_bytes = total_bytes,
        limit_bytes = limit_bytes,
        "Disk space check"
    );

    if total_bytes <= limit_bytes {
        return deleted;
    }

    let mut excess = total_bytes - limit_bytes;

    // Delete oldest uploaded files first (sorted by mtime ascending — files already sorted)
    for file in &remaining {
        if excess == 0 {
            break;
        }
        if file.is_active {
            continue; // Never delete active file
        }
        if !uploaded_hashes.contains(&file.content_hash) {
            continue; // Skip unuploaded in this pass
        }
        if !file.path.exists() {
            continue; // Externally deleted
        }
        // Rotation-aware: verify content hash still matches before deleting
        if let Ok(current_hash) = compute_content_hash(&file.path) {
            if current_hash != file.content_hash {
                tracing::warn!(
                    "File rotated (hash changed), skipping deletion: {}",
                    file.path.display()
                );
                continue;
            }
        }
        if let Ok(meta) = fs::metadata(&file.path) {
            let size = meta.len();
            if fs::remove_file(&file.path).is_ok() {
                tracing::warn!(
                    "Disk limit exceeded, deleting uploaded file: {}",
                    file.path.display()
                );
                deleted.insert(file.path.clone());
                excess = excess.saturating_sub(size);
            }
        }
    }

    // If still over limit, delete oldest unuploaded files (accepting data loss)
    if excess > 0 {
        for file in &remaining {
            if excess == 0 {
                break;
            }
            if file.is_active || deleted.contains(&file.path) {
                continue;
            }
            if !file.path.exists() {
                continue; // Externally deleted
            }
            if let Ok(meta) = fs::metadata(&file.path) {
                let size = meta.len();
                if fs::remove_file(&file.path).is_ok() {
                    tracing::warn!(
                        "Disk limit exceeded, deleting unuploaded file (data loss): {}",
                        file.path.display()
                    );
                    deleted.insert(file.path.clone());
                    excess = excess.saturating_sub(size);
                }
            }
        }
    }

    deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use std::time::SystemTime;

    fn make_file(dir: &Path, name: &str, size: usize) -> ScannedFile {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(&vec![b'x'; size]).unwrap();
        ScannedFile {
            path,
            mtime: SystemTime::now(),
            content_hash: format!("hash_{name}"),
            is_active: false,
        }
    }

    #[test]
    fn test_parse_limit_bytes() {
        assert_eq!(parse_limit_bytes("100", "KB").unwrap(), 102400);
        assert_eq!(parse_limit_bytes("1", "MB").unwrap(), 1048576);
        assert_eq!(parse_limit_bytes("1", "GB").unwrap(), 1073741824);
        assert!(parse_limit_bytes("abc", "MB").is_err());
        assert!(parse_limit_bytes("1", "TB").is_err());
    }

    #[test]
    fn test_parse_limit_bytes_zero() {
        // Zero is valid for parsing (validation happens elsewhere)
        assert_eq!(parse_limit_bytes("0", "KB").unwrap(), 0);
        assert_eq!(parse_limit_bytes("0", "MB").unwrap(), 0);
        assert_eq!(parse_limit_bytes("0", "GB").unwrap(), 0);
    }

    #[test]
    fn test_under_limit_no_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = make_file(dir.path(), "a.json", 100);
        let f2 = make_file(dir.path(), "b.json", 100);
        let files = vec![f1, f2];

        let deleted = enforce_disk_limit(&files, 1024, &HashSet::new(), false);
        assert!(deleted.is_empty());
    }

    #[test]
    fn test_over_limit_deletes_uploaded_first() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = make_file(dir.path(), "a.json", 500);
        let f2 = make_file(dir.path(), "b.json", 500);
        let mut f3 = make_file(dir.path(), "c.json", 500);
        f3.is_active = true;

        let mut uploaded = HashSet::new();
        uploaded.insert("hash_a.json".to_string());

        let files = vec![f1, f2, f3];
        // Total 1500 bytes, limit 1024 — need to delete ~476 bytes
        let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);
        assert_eq!(deleted.len(), 1);
        assert!(deleted.iter().any(|p| p.ends_with("a.json"))); // Uploaded file deleted first
    }

    #[test]
    fn test_active_file_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut f1 = make_file(dir.path(), "active.json", 2000);
        f1.is_active = true;

        let files = vec![f1];
        let deleted = enforce_disk_limit(&files, 100, &HashSet::new(), false);
        assert!(deleted.is_empty()); // Active file protected
    }

    #[test]
    fn test_delete_after_upload_flag() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = make_file(dir.path(), "a.json", 100);
        let mut f2 = make_file(dir.path(), "b.json", 100);
        f2.is_active = true;

        let mut uploaded = HashSet::new();
        uploaded.insert("hash_a.json".to_string());

        let files = vec![f1, f2];
        // Under limit, but delete_after_upload is true
        let deleted = enforce_disk_limit(&files, 10000, &uploaded, true);
        assert_eq!(deleted.len(), 1);
        assert!(deleted.iter().any(|p| p.ends_with("a.json")));
    }

    #[test]
    fn test_delete_after_upload_protects_active() {
        let dir = tempfile::tempdir().unwrap();
        let mut f1 = make_file(dir.path(), "active.json", 100);
        f1.is_active = true;

        let mut uploaded = HashSet::new();
        uploaded.insert("hash_active.json".to_string());

        let files = vec![f1];
        let deleted = enforce_disk_limit(&files, 10000, &uploaded, true);
        assert!(deleted.is_empty()); // Active file never deleted even if uploaded
    }

    #[test]
    fn test_over_limit_only_unuploaded_files() {
        // When over limit and no uploaded files exist, must delete unuploaded (data loss)
        let dir = tempfile::tempdir().unwrap();
        let f1 = make_file(dir.path(), "a.json", 500);
        let f2 = make_file(dir.path(), "b.json", 500);
        let mut f3 = make_file(dir.path(), "c.json", 500);
        f3.is_active = true;

        // No uploaded files
        let uploaded = HashSet::new();

        let files = vec![f1, f2, f3];
        // Total 1500 bytes, limit 1024 — need to delete ~476 bytes
        // Since no uploaded files, must delete unuploaded
        let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);
        assert!(!deleted.is_empty());
        // Should delete oldest non-active file
        assert!(deleted
            .iter()
            .any(|p| p.ends_with("a.json") || p.ends_with("b.json")));
        // Active file should NOT be deleted
        assert!(!deleted.iter().any(|p| p.ends_with("c.json")));
    }

    #[test]
    fn test_externally_deleted_file_no_error() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = make_file(dir.path(), "a.json", 500);
        let f2 = make_file(dir.path(), "b.json", 500);
        let mut f3 = make_file(dir.path(), "c.json", 500);
        f3.is_active = true;

        let mut uploaded = HashSet::new();
        uploaded.insert("hash_a.json".to_string());

        // Externally delete the file before enforce_disk_limit runs
        fs::remove_file(&f1.path).unwrap();

        let files = vec![f1.clone(), f2, f3];
        // Should not panic or error when file is already gone
        let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);
        assert!(!deleted.contains(&f1.path));
    }

    #[test]
    fn test_rotated_file_not_deleted_if_hash_changed() {
        let dir = tempfile::tempdir().unwrap();
        let mut f1 = make_file(dir.path(), "a.json", 500);
        let f2 = make_file(dir.path(), "b.json", 500);
        let mut f3 = make_file(dir.path(), "c.json", 500);
        f3.is_active = true;

        let mut uploaded = HashSet::new();
        uploaded.insert(f1.content_hash.clone());

        // Simulate file rotation: overwrite with different content (changes hash)
        fs::write(&f1.path, "completely different content after rotation").unwrap();

        let files = vec![f1.clone(), f2, f3];
        let deleted = enforce_disk_limit(&files, 1024, &uploaded, false);
        // Rotated file should NOT be deleted because hash no longer matches
        assert!(!deleted.contains(&f1.path));
    }

    #[test]
    fn test_zero_limit_disables_disk_management() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = make_file(dir.path(), "a.json", 500);
        let mut uploaded = HashSet::new();
        uploaded.insert("hash_a.json".to_string());

        let files = vec![f1.clone()];
        let deleted = enforce_disk_limit(&files, 0, &uploaded, true);
        assert!(deleted.is_empty());
        assert!(f1.path.exists());
    }

    #[test]
    fn test_config_removal_clears_uploaded_set() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = make_file(dir.path(), "a.json", 100);
        let files = vec![f1.clone()];
        // Empty uploaded set (simulates config removal)
        let deleted = enforce_disk_limit(&files, 1024, &HashSet::new(), false);
        assert!(deleted.is_empty());
        assert!(f1.path.exists());
    }
}
