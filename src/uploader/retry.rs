// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Retry logic with exponential backoff

use super::cw_client::{CwLogsClient, CwUploadError};
use super::SealedBatch;
use std::time::Duration;
use tokio::time::sleep;

const MAX_RETRIES: usize = 5;
const BACKOFF_DELAYS: [u64; 5] = [1, 2, 4, 8, 16];

/// Upload a batch with exponential backoff retry.
/// Returns Ok(true) if upload succeeded, Ok(false) if all retries exhausted.
#[cfg(not(tarpaulin_include))]
pub async fn upload_with_retry(
    client: &mut CwLogsClient,
    batch: &SealedBatch,
) -> Result<bool, String> {
    let mut attempt = 0;
    let mut token_retries = 0;
    loop {
        match client.upload_batch(batch).await {
            Ok(()) => return Ok(true),
            Err(CwUploadError::InvalidSequenceToken { expected }) => {
                token_retries += 1;
                if token_retries >= 3 {
                    return Err("InvalidSequenceToken persists after 3 retries".to_string());
                }
                tracing::info!("InvalidSequenceToken, retrying with token: {}", expected);
                client.set_sequence_token(&batch.log_group, &batch.log_stream, Some(expected));
                // Immediate retry — does NOT increment attempt
                continue;
            }
            Err(CwUploadError::AuthError) => {
                token_retries = 0;
                tracing::warn!("Auth error on attempt {}, retrying (SDK handles credential refresh internally)", attempt);
                if attempt >= 1 {
                    return Err("Auth error persists after retry".to_string());
                }
            }
            Err(CwUploadError::Throttled) => {
                token_retries = 0;
                if attempt >= MAX_RETRIES - 1 {
                    tracing::error!(
                        "Upload failed after {} retries for log group {}",
                        MAX_RETRIES,
                        batch.log_group
                    );
                    return Ok(false);
                }
                let delay = BACKOFF_DELAYS[attempt.min(BACKOFF_DELAYS.len() - 1)];
                tracing::warn!("Throttled on attempt {}, backing off {}s", attempt, delay);
                sleep(Duration::from_secs(delay)).await;
            }
            Err(CwUploadError::Retriable(msg)) => {
                token_retries = 0;
                if attempt >= MAX_RETRIES - 1 {
                    tracing::error!("Retriable error after {} retries: {}", MAX_RETRIES, msg);
                    return Ok(false);
                }
                let delay = BACKOFF_DELAYS[attempt.min(BACKOFF_DELAYS.len() - 1)];
                tracing::warn!("Retriable error on attempt {}: {}, backing off {}s", attempt, msg, delay);
                sleep(Duration::from_secs(delay)).await;
            }
            Err(CwUploadError::Other(msg)) => {
                token_retries = 0;
                if attempt >= MAX_RETRIES - 1 {
                    tracing::error!("Upload failed after {} retries: {}", MAX_RETRIES, msg);
                    return Ok(false);
                }
                let delay = BACKOFF_DELAYS[attempt.min(BACKOFF_DELAYS.len() - 1)];
                tracing::warn!(
                    "Upload error on attempt {}: {}, backing off {}s",
                    attempt,
                    msg,
                    delay
                );
                sleep(Duration::from_secs(delay)).await;
            }
        }
        attempt += 1;
    }
}

/// Format log stream name: /{year}/{month:02}/{day:02}/thing/{thingName}
/// Replaces colons with plus signs since CW log stream names cannot contain colons.
pub fn format_log_stream_name(thing_name: &str) -> String {
    let now = chrono_free_utc_date();
    let safe_name = thing_name.replace(':', "+");
    format!("/{}/{:02}/{:02}/thing/{}", now.0, now.1, now.2, safe_name)
}

/// Get current UTC date as (year, month, day) without chrono dependency
fn chrono_free_utc_date() -> (i32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs / 86400;
    // Civil date from days since epoch (algorithm from Howard Hinnant)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

/// Check if a timestamp falls on a different UTC date than the stream date
pub fn is_different_date(
    timestamp_ms: i64,
    stream_year: i32,
    stream_month: u32,
    stream_day: u32,
) -> bool {
    if timestamp_ms < 0 {
        return true; // Treat negative timestamps as different date
    }
    let secs = timestamp_ms / 1000;
    let days = secs / 86400;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    y as i32 != stream_year || m != stream_month || d != stream_day
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_log_stream_name() {
        let name = format_log_stream_name("eca-store66-device01");
        assert!(name.starts_with('/'));
        assert!(name.ends_with("/thing/eca-store66-device01"));
        // Verify format: /YYYY/MM/DD/thing/name
        let parts: Vec<&str> = name.split('/').collect();
        assert_eq!(parts.len(), 6); // ["", "YYYY", "MM", "DD", "thing", "name"]
        assert_eq!(parts[4], "thing");
    }

    #[test]
    fn test_format_log_stream_name_with_colons() {
        // Colons should be replaced with plus signs
        let name = format_log_stream_name("device:with:colons");
        assert!(name.ends_with("/thing/device+with+colons"));
        assert!(!name.contains(':'));
    }

    #[test]
    fn test_chrono_free_utc_date() {
        let (y, m, d) = chrono_free_utc_date();
        assert!(y >= 2024);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
    }

    #[test]
    fn test_is_different_date_same() {
        let (y, m, d) = chrono_free_utc_date();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(!is_different_date(now_ms, y, m, d));
    }

    #[test]
    fn test_is_different_date_different() {
        // Jan 1 2024 00:00:00 UTC = 1704067200000 ms
        assert!(is_different_date(1704067200000, 2023, 12, 31));
        assert!(!is_different_date(1704067200000, 2024, 1, 1));
    }

    #[test]
    fn test_is_different_date_negative_timestamp() {
        // Negative timestamps should be treated as different date
        assert!(is_different_date(-1, 2024, 1, 1));
        assert!(is_different_date(-1000000, 2024, 1, 1));
    }

    #[test]
    fn test_is_different_date_midnight_boundary() {
        // Dec 31 2023 23:59:59.999 UTC = 1704067199999 ms
        assert!(!is_different_date(1704067199999, 2023, 12, 31));
        // Jan 1 2024 00:00:00.000 UTC = 1704067200000 ms
        assert!(!is_different_date(1704067200000, 2024, 1, 1));
        // Cross-boundary check
        assert!(is_different_date(1704067199999, 2024, 1, 1));
        assert!(is_different_date(1704067200000, 2023, 12, 31));
    }

    #[test]
    fn test_backoff_delays() {
        assert_eq!(BACKOFF_DELAYS, [1, 2, 4, 8, 16]);
        assert_eq!(MAX_RETRIES, 5);
    }
}
