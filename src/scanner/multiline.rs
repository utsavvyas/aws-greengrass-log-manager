// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Multi-line log assembly

use super::reader::{extract_timestamp, LogEvent};
use regex::Regex;

/// Maximum event size in bytes (CloudWatch Logs limit: 256KB - 8 timestamp - 26 overhead)
const MAX_EVENT_SIZE: usize = 262_110;

/// Assemble lines into LogEvents based on multi-line start pattern.
/// When start_pattern is None, each line becomes its own LogEvent.
/// When start_pattern is Some, buffer lines until a new match, emit buffered joined by '\n'.
pub fn assemble_multiline(
    lines: Vec<String>,
    start_pattern: Option<&Regex>,
    default_timestamp: i64,
) -> Vec<LogEvent> {
    let events = match start_pattern {
        None => lines
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| LogEvent {
                timestamp: extract_timestamp(&l).unwrap_or(default_timestamp),
                message: l,
            })
            .collect(),
        Some(pattern) => {
            let mut events = Vec::new();
            let mut buffer: Vec<String> = Vec::new();

            for line in lines {
                if pattern.is_match(&line) && !buffer.is_empty() {
                    events.push(emit_buffered(&buffer, default_timestamp));
                    buffer.clear();
                }
                if !line.trim().is_empty() {
                    buffer.push(line);
                }
            }
            if !buffer.is_empty() {
                events.push(emit_buffered(&buffer, default_timestamp));
            }
            events
        }
    };
    tracing::debug!(event_count = events.len(), "Multiline assembly complete");
    events
}

fn emit_buffered(buffer: &[String], default_timestamp: i64) -> LogEvent {
    let timestamp = extract_timestamp(&buffer[0]).unwrap_or(default_timestamp);
    let joined = buffer.join("\n");
    let message = if joined.len() <= MAX_EVENT_SIZE {
        joined
    } else {
        tracing::warn!(
            original_size = joined.len(),
            max_size = MAX_EVENT_SIZE,
            "Truncating oversized multiline event"
        );
        let mut end = MAX_EVENT_SIZE;
        while !joined.is_char_boundary(end) {
            end -= 1;
        }
        joined[..end].to_string()
    };
    LogEvent { timestamp, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_line_mode_none_pattern() {
        let lines = vec!["line1".into(), "line2".into(), "line3".into()];
        let events = assemble_multiline(lines, None, 1000);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].message, "line1");
        assert_eq!(events[1].message, "line2");
        assert_eq!(events[2].message, "line3");
        assert!(events.iter().all(|e| e.timestamp == 1000));
    }

    #[test]
    fn test_multiline_assembly() {
        let pattern = Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap();
        let lines = vec![
            "2024-01-15 ERROR something".into(),
            "  stack trace line 1".into(),
            "  stack trace line 2".into(),
            "2024-01-16 INFO next event".into(),
        ];
        let events = assemble_multiline(lines, Some(&pattern), 1000);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].message,
            "2024-01-15 ERROR something\n  stack trace line 1\n  stack trace line 2"
        );
        assert_eq!(events[1].message, "2024-01-16 INFO next event");
    }

    #[test]
    fn test_eof_flush() {
        let pattern = Regex::new(r"^START").unwrap();
        let lines = vec!["START event1".into(), "continuation".into()];
        let events = assemble_multiline(lines, Some(&pattern), 1000);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, "START event1\ncontinuation");
    }

    #[test]
    fn test_empty_input() {
        let events = assemble_multiline(vec![], None, 1000);
        assert!(events.is_empty());

        let pattern = Regex::new(r"^START").unwrap();
        let events = assemble_multiline(vec![], Some(&pattern), 1000);
        assert!(events.is_empty());
    }

    #[test]
    fn test_empty_lines_skipped() {
        let lines = vec!["line1".into(), "".into(), "   ".into(), "line2".into()];
        let events = assemble_multiline(lines, None, 1000);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_timestamp_extraction_in_multiline() {
        let pattern = Regex::new(r"^\d{13}").unwrap();
        let lines = vec![
            "1705314645000 first event".into(),
            "continuation".into(),
            "1705314646000 second event".into(),
        ];
        let events = assemble_multiline(lines, Some(&pattern), 9999);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].timestamp, 1705314645000);
        assert_eq!(events[1].timestamp, 1705314646000);
    }

    #[test]
    fn test_timestamp_fallback_to_default() {
        let lines = vec!["no timestamp here".into()];
        let events = assemble_multiline(lines, None, 5555);
        assert_eq!(events[0].timestamp, 5555);
    }

    #[test]
    fn test_orphan_continuation_lines() {
        // Lines that don't match the pattern at the start (orphan continuations)
        let pattern = Regex::new(r"^START").unwrap();
        let lines = vec![
            "  orphan continuation 1".into(),
            "  orphan continuation 2".into(),
            "START actual event".into(),
        ];
        let events = assemble_multiline(lines, Some(&pattern), 1000);
        // Orphan lines should be buffered and emitted when START is seen
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].message,
            "  orphan continuation 1\n  orphan continuation 2"
        );
        assert_eq!(events[1].message, "START actual event");
    }

    #[test]
    fn test_only_orphan_lines() {
        // All lines are orphans (none match the pattern)
        let pattern = Regex::new(r"^START").unwrap();
        let lines = vec!["  orphan line 1".into(), "  orphan line 2".into()];
        let events = assemble_multiline(lines, Some(&pattern), 1000);
        // Should emit all orphans as a single event at EOF
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, "  orphan line 1\n  orphan line 2");
        assert_eq!(events[0].timestamp, 1000); // Uses default timestamp
    }

    #[test]
    fn test_oversized_multiline_truncation() {
        // Create a multiline event that exceeds MAX_EVENT_SIZE when joined
        let pattern = Regex::new(r"^START").unwrap();
        let large_line = "x".repeat(200_000);
        let lines = vec!["START event".into(), large_line.clone(), large_line];
        let events = assemble_multiline(lines, Some(&pattern), 1000);
        assert_eq!(events.len(), 1);
        // Message should be truncated to MAX_EVENT_SIZE
        assert!(events[0].message.len() <= 262_110);
    }

    #[test]
    fn test_regex_pathological_pattern_no_crash() {
        // Java had stack overflow with certain patterns — Rust regex is bounded
        let pattern = Regex::new(r"^(a+)+$").unwrap();
        let lines = vec!["aaaaaaaaaaaaaaaaab".into()];
        let events = assemble_multiline(lines, Some(&pattern), 1000);
        // Should complete without hanging or crashing
        assert!(!events.is_empty());
    }

    #[test]
    fn test_orphan_lines_flushed_on_pattern_change() {
        // Verifies that if the multiline pattern changes, buffered orphan lines are flushed correctly
        let pattern1 = Regex::new(r"^START").unwrap();
        let lines1 = vec!["  orphan line".into(), "START event".into()];
        let events1 = assemble_multiline(lines1, Some(&pattern1), 1000);
        assert_eq!(events1.len(), 2);

        // Simulate config change to a different pattern
        let pattern2 = Regex::new(r"^BEGIN").unwrap();
        let lines2 = vec!["  leftover orphan".into(), "BEGIN new event".into()];
        let events2 = assemble_multiline(lines2, Some(&pattern2), 2000);
        // Orphan should be flushed when new pattern match is seen
        assert_eq!(events2.len(), 2);
        assert_eq!(events2[0].message, "  leftover orphan");
        assert_eq!(events2[1].message, "BEGIN new event");
    }
}
