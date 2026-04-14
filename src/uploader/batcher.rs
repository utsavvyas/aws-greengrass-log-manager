// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Log event batching for PutLogEvents

use crate::scanner::LogEvent;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BATCH_BYTES: usize = 1_048_576;
const MAX_BATCH_EVENTS: usize = 10_000;
const PER_EVENT_OVERHEAD: usize = 26;
const MAX_EVENT_SIZE: usize = 262_110;
const FOURTEEN_DAYS_MS: i64 = 14 * 24 * 60 * 60 * 1000;
const TWO_HOURS_MS: i64 = 2 * 60 * 60 * 1000;

#[derive(Debug)]
pub struct SealedBatch {
    pub log_group: String,
    pub log_stream: String,
    pub events: Vec<LogEvent>,
}

pub fn batch_events(
    log_group: &str,
    log_stream: &str,
    mut events: Vec<LogEvent>,
) -> Vec<SealedBatch> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let min_ts = now_ms - FOURTEEN_DAYS_MS;
    let max_ts = now_ms + TWO_HOURS_MS;

    events.retain(|e| {
        if e.timestamp < min_ts || e.timestamp > max_ts {
            tracing::warn!(
                "Dropping event with out-of-range timestamp {} (allowed {}-{})",
                e.timestamp,
                min_ts,
                max_ts
            );
            false
        } else {
            true
        }
    });

    events.sort_by_key(|e| e.timestamp);

    let mut batches = Vec::new();
    let mut current_events = Vec::new();
    let mut current_bytes: usize = 0;

    for event in events {
        let event_size = event.message.len() + PER_EVENT_OVERHEAD;
        if event.message.len() > MAX_EVENT_SIZE {
            // Split oversized event into chunks (matching Java LogManager behavior)
            let msg = &event.message;
            let mut start = 0;
            while start < msg.len() {
                let mut end = (start + MAX_EVENT_SIZE).min(msg.len());
                while !msg.is_char_boundary(end) {
                    end -= 1;
                }
                let chunk = LogEvent {
                    timestamp: event.timestamp,
                    message: msg[start..end].to_string(),
                };
                let chunk_size = chunk.message.len() + PER_EVENT_OVERHEAD;
                if !current_events.is_empty()
                    && (current_bytes + chunk_size > MAX_BATCH_BYTES
                        || current_events.len() >= MAX_BATCH_EVENTS)
                {
                    tracing::debug!(
                        event_count = current_events.len(),
                        batch_bytes = current_bytes,
                        "Batch sealed"
                    );
                    batches.push(SealedBatch {
                        log_group: log_group.to_string(),
                        log_stream: log_stream.to_string(),
                        events: std::mem::take(&mut current_events),
                    });
                    current_bytes = 0;
                }
                current_bytes += chunk_size;
                current_events.push(chunk);
                start = end;
            }
            continue;
        }
        if !current_events.is_empty()
            && (current_bytes + event_size > MAX_BATCH_BYTES
                || current_events.len() >= MAX_BATCH_EVENTS)
        {
            tracing::debug!(
                event_count = current_events.len(),
                batch_bytes = current_bytes,
                "Batch sealed"
            );
            batches.push(SealedBatch {
                log_group: log_group.to_string(),
                log_stream: log_stream.to_string(),
                events: std::mem::take(&mut current_events),
            });
            current_bytes = 0;
        }
        current_bytes += event_size;
        current_events.push(event);
    }

    if !current_events.is_empty() {
        tracing::debug!(
            event_count = current_events.len(),
            batch_bytes = current_bytes,
            "Batch sealed"
        );
        batches.push(SealedBatch {
            log_group: log_group.to_string(),
            log_stream: log_stream.to_string(),
            events: current_events,
        });
    }

    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn make_event(ts: i64, msg: &str) -> LogEvent {
        LogEvent {
            timestamp: ts,
            message: msg.to_string(),
        }
    }

    #[test]
    fn test_sorting() {
        let now = now_ms();
        let events = vec![
            make_event(now + 100, "c"),
            make_event(now - 100, "a"),
            make_event(now, "b"),
        ];
        let batches = batch_events("grp", "stream", events);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].events[0].message, "a");
        assert_eq!(batches[0].events[1].message, "b");
        assert_eq!(batches[0].events[2].message, "c");
    }

    #[test]
    fn test_batch_split_by_event_count() {
        let now = now_ms();
        let events: Vec<LogEvent> = (0..MAX_BATCH_EVENTS + 5)
            .map(|i| make_event(now + i as i64, "x"))
            .collect();
        let batches = batch_events("grp", "stream", events);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].events.len(), MAX_BATCH_EVENTS);
        assert_eq!(batches[1].events.len(), 5);
    }

    #[test]
    fn test_batch_split_by_size() {
        let now = now_ms();
        // Each event: 1000 bytes msg + 26 overhead = 1026 bytes
        // 1,048,576 / 1026 = ~1021 events per batch
        let msg = "x".repeat(1000);
        let events: Vec<LogEvent> = (0..2000)
            .map(|i| make_event(now + i as i64, &msg))
            .collect();
        let batches = batch_events("grp", "stream", events);
        assert!(batches.len() >= 2);
        for batch in &batches {
            let total: usize = batch
                .events
                .iter()
                .map(|e| e.message.len() + PER_EVENT_OVERHEAD)
                .sum();
            assert!(total <= MAX_BATCH_BYTES);
        }
    }

    #[test]
    fn test_timestamp_filtering() {
        let now = now_ms();
        let events = vec![
            make_event(now - FOURTEEN_DAYS_MS - 1000, "too old"),
            make_event(now, "valid"),
            make_event(now + TWO_HOURS_MS + 1000, "too future"),
        ];
        let batches = batch_events("grp", "stream", events);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].events.len(), 1);
        assert_eq!(batches[0].events[0].message, "valid");
    }

    #[test]
    fn test_empty_input() {
        let batches = batch_events("grp", "stream", vec![]);
        assert!(batches.is_empty());
    }

    #[test]
    fn test_oversized_event_chunking() {
        let now = now_ms();
        // Create an event larger than MAX_EVENT_SIZE (262110 bytes)
        let large_msg = "x".repeat(300_000);
        let events = vec![make_event(now, &large_msg)];
        let batches = batch_events("grp", "stream", events);

        // Should produce multiple events, each <= MAX_EVENT_SIZE
        assert!(!batches.is_empty());
        let total_events: usize = batches.iter().map(|b| b.events.len()).sum();
        assert!(total_events >= 2); // 300k / 262110 = at least 2 chunks

        for batch in &batches {
            for event in &batch.events {
                assert!(event.message.len() <= MAX_EVENT_SIZE);
            }
        }
    }

    #[test]
    fn test_oversized_event_preserves_timestamp() {
        let now = now_ms();
        let large_msg = "x".repeat(300_000);
        let events = vec![make_event(now, &large_msg)];
        let batches = batch_events("grp", "stream", events);

        // All chunks should have the same timestamp
        for batch in &batches {
            for event in &batch.events {
                assert_eq!(event.timestamp, now);
            }
        }
    }

    #[test]
    fn test_oversized_event_with_multibyte_chars() {
        let now = now_ms();
        // Create a message with multibyte UTF-8 characters that would be split
        // at a non-char-boundary if we just truncated at MAX_EVENT_SIZE
        let emoji = "🎉"; // 4 bytes each
        let base = emoji.repeat(100_000); // 400,000 bytes
        let events = vec![make_event(now, &base)];
        let batches = batch_events("grp", "stream", events);

        // Should produce multiple chunks, each valid UTF-8
        assert!(!batches.is_empty());
        for batch in &batches {
            for event in &batch.events {
                assert!(event.message.len() <= MAX_EVENT_SIZE);
                // Verify it's valid UTF-8 (would panic if not)
                let _ = event.message.chars().count();
            }
        }
    }

    #[test]
    fn test_oversized_event_causes_batch_split() {
        let now = now_ms();
        // First, fill up a batch with normal events
        let normal_msg = "x".repeat(200_000);
        let mut events: Vec<LogEvent> = (0..5)
            .map(|i| make_event(now + i as i64, &normal_msg))
            .collect();
        // Then add an oversized event that will be chunked
        // The chunks should cause a new batch to be created
        let large_msg = "y".repeat(500_000);
        events.push(make_event(now + 100, &large_msg));

        let batches = batch_events("grp", "stream", events);

        // Should have multiple batches due to size limits
        assert!(batches.len() >= 2);
        // Verify all events are within limits
        for batch in &batches {
            let total: usize = batch
                .events
                .iter()
                .map(|e| e.message.len() + PER_EVENT_OVERHEAD)
                .sum();
            assert!(total <= MAX_BATCH_BYTES);
        }
    }
}
