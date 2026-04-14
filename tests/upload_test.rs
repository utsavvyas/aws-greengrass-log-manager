// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for upload pipeline with batch splitting

use gg_log_manager::scanner::LogEvent;
use gg_log_manager::uploader::batch_events;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn test_batch_splitting_exceeds_10000_events() {
    let now = now_ms();

    // Create 15,000 log events (exceeding the 10,000-event batch limit)
    let events: Vec<LogEvent> = (0..15_000)
        .map(|i| make_event(now + i as i64, &format!("event_{}", i)))
        .collect();

    let batches = batch_events("test-log-group", "test-stream", events);

    // Should produce 2 batches: first with 10,000 events, second with 5,000
    assert_eq!(
        batches.len(),
        2,
        "Should produce exactly 2 batches for 15,000 events"
    );
    assert_eq!(
        batches[0].events.len(),
        10_000,
        "First batch should have 10,000 events"
    );
    assert_eq!(
        batches[1].events.len(),
        5_000,
        "Second batch should have 5,000 events"
    );
}

#[test]
fn test_events_sorted_by_timestamp() {
    let now = now_ms();

    // Create events with out-of-order timestamps
    let events = vec![
        make_event(now + 500, "event_c"),
        make_event(now + 100, "event_a"),
        make_event(now + 300, "event_b"),
        make_event(now + 200, "event_d"),
        make_event(now + 400, "event_e"),
    ];

    let batches = batch_events("test-log-group", "test-stream", events);

    assert_eq!(batches.len(), 1);
    let batch = &batches[0];

    // Verify events are sorted by timestamp ascending
    for i in 1..batch.events.len() {
        assert!(
            batch.events[i - 1].timestamp <= batch.events[i].timestamp,
            "Events should be sorted by timestamp: {} should be <= {}",
            batch.events[i - 1].timestamp,
            batch.events[i].timestamp
        );
    }

    // Verify the order matches expected
    assert_eq!(batch.events[0].message, "event_a");
    assert_eq!(batch.events[1].message, "event_d");
    assert_eq!(batch.events[2].message, "event_b");
    assert_eq!(batch.events[3].message, "event_e");
    assert_eq!(batch.events[4].message, "event_c");
}

#[test]
fn test_batch_metadata_preserved() {
    let now = now_ms();
    let events = vec![make_event(now, "test")];

    let batches = batch_events("my-log-group", "my-stream", events);

    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].log_group, "my-log-group");
    assert_eq!(batches[0].log_stream, "my-stream");
}

#[test]
fn test_empty_events_produces_no_batches() {
    let batches = batch_events("test-group", "test-stream", vec![]);
    assert!(batches.is_empty());
}

#[test]
fn test_batch_splitting_by_size() {
    let now = now_ms();

    // Create events with large messages to trigger size-based splitting
    // Each event: 10KB message + 26 bytes overhead = ~10KB
    // Max batch size is 1MB, so ~100 events per batch
    let large_msg = "x".repeat(10_000);
    let events: Vec<LogEvent> = (0..200)
        .map(|i| make_event(now + i as i64, &large_msg))
        .collect();

    let batches = batch_events("test-group", "test-stream", events);

    // Should produce multiple batches due to size limit
    assert!(
        batches.len() >= 2,
        "Should produce multiple batches for large events"
    );

    // Each batch should respect the 1MB limit
    const MAX_BATCH_BYTES: usize = 1_048_576;
    const PER_EVENT_OVERHEAD: usize = 26;

    for batch in &batches {
        let total_size: usize = batch
            .events
            .iter()
            .map(|e| e.message.len() + PER_EVENT_OVERHEAD)
            .sum();
        assert!(
            total_size <= MAX_BATCH_BYTES,
            "Batch size {} exceeds limit {}",
            total_size,
            MAX_BATCH_BYTES
        );
    }
}

#[test]
fn test_sorting_preserved_across_batches() {
    let now = now_ms();

    // Create 15,000 events with decreasing timestamps (worst case for sorting)
    let events: Vec<LogEvent> = (0..15_000)
        .map(|i| make_event(now + (15_000 - i) as i64, &format!("event_{}", i)))
        .collect();

    let batches = batch_events("test-group", "test-stream", events);

    assert_eq!(batches.len(), 2);

    // Verify sorting within each batch
    for batch in &batches {
        for i in 1..batch.events.len() {
            assert!(
                batch.events[i - 1].timestamp <= batch.events[i].timestamp,
                "Events within batch should be sorted"
            );
        }
    }

    // Verify sorting across batches (last event of batch 0 <= first event of batch 1)
    let last_of_first = batches[0].events.last().unwrap().timestamp;
    let first_of_second = batches[1].events.first().unwrap().timestamp;
    assert!(
        last_of_first <= first_of_second,
        "Events should be sorted across batches"
    );
}

#[test]
fn test_exactly_10000_events_single_batch() {
    let now = now_ms();

    let events: Vec<LogEvent> = (0..10_000)
        .map(|i| make_event(now + i as i64, "x"))
        .collect();

    let batches = batch_events("test-group", "test-stream", events);

    assert_eq!(
        batches.len(),
        1,
        "Exactly 10,000 events should fit in one batch"
    );
    assert_eq!(batches[0].events.len(), 10_000);
}

#[test]
fn test_10001_events_two_batches() {
    let now = now_ms();

    let events: Vec<LogEvent> = (0..10_001)
        .map(|i| make_event(now + i as i64, "x"))
        .collect();

    let batches = batch_events("test-group", "test-stream", events);

    assert_eq!(batches.len(), 2, "10,001 events should produce 2 batches");
    assert_eq!(batches[0].events.len(), 10_000);
    assert_eq!(batches[1].events.len(), 1);
}
