//! Technical input-token cache accounting.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::types::{Message, MessagesRequest};

const MAX_SAVINGS_RATIO: f64 = 0.90;
const MIN_SEGMENT_CHARS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InputCacheReport {
    pub raw_input_tokens: i64,
    pub estimated_billable_input_tokens: i64,
    pub saved_input_tokens: i64,
    pub input_cache_hit_rate: f64,
    pub prefix_cache_state: String,
    pub tool_result_cache_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cache_ttl_secs: Option<u64>,
}

impl InputCacheReport {
    pub(crate) fn bypass(raw_input_tokens: i64) -> Self {
        Self {
            raw_input_tokens,
            estimated_billable_input_tokens: raw_input_tokens,
            saved_input_tokens: 0,
            input_cache_hit_rate: 0.0,
            prefix_cache_state: "bypass".to_string(),
            tool_result_cache_state: "bypass".to_string(),
            input_cache_ttl_secs: None,
        }
    }
}

pub(crate) struct InputCache {
    root: PathBuf,
    short_ttl_secs: u64,
    long_ttl_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    Prefix,
    MessagePrefix,
    ToolResult,
}

#[derive(Debug)]
struct Segment {
    kind: SegmentKind,
    name: &'static str,
    ttl_secs: u64,
    byte_len: usize,
    token_estimate: i64,
    key: String,
    should_store: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct SegmentEntry {
    cached_at: u64,
    ttl_secs: u64,
    token_estimate: i64,
    kind: String,
    name: String,
}

impl InputCache {
    pub(crate) fn new(root: PathBuf, short_ttl_secs: u64, long_ttl_secs: u64) -> Self {
        Self {
            root,
            short_ttl_secs,
            long_ttl_secs,
        }
    }

    pub(crate) fn analyze_and_store(
        &self,
        request: &MessagesRequest,
        raw_input_tokens: i64,
    ) -> InputCacheReport {
        self.analyze_and_store_at(request, raw_input_tokens, now_unix_secs())
    }

    pub(crate) fn analyze_and_store_at(
        &self,
        request: &MessagesRequest,
        raw_input_tokens: i64,
        now_secs: u64,
    ) -> InputCacheReport {
        if raw_input_tokens <= 0 {
            return InputCacheReport::bypass(0);
        }

        let segments = self.extract_segments(request);
        if segments.is_empty() {
            return InputCacheReport::bypass(raw_input_tokens);
        }

        let mut saved_input_tokens = 0_i64;
        let mut best_message_prefix_hit_tokens = 0_i64;
        let mut prefix_saw = false;
        let mut prefix_hit = false;
        let mut prefix_stored = false;
        let mut tool_saw = false;
        let mut tool_hit = false;
        let mut tool_stored = false;
        let mut observed_ttl_secs: Option<u64> = None;
        let full_message_prefix_bytes = segments
            .iter()
            .filter(|segment| segment.kind == SegmentKind::MessagePrefix && segment.should_store)
            .map(|segment| segment.byte_len)
            .max()
            .unwrap_or(0);

        for segment in &segments {
            observed_ttl_secs = Some(observed_ttl_secs.unwrap_or(0).max(segment.ttl_secs));
            let hit = self.segment_is_fresh(&segment, now_secs);

            match segment.kind {
                SegmentKind::Prefix => {
                    prefix_saw = true;
                    prefix_hit |= hit;
                    prefix_stored |= !hit;
                    if hit {
                        saved_input_tokens =
                            saved_input_tokens.saturating_add(segment.token_estimate);
                    }
                }
                SegmentKind::MessagePrefix => {
                    prefix_saw = true;
                    prefix_hit |= hit;
                    prefix_stored |= !hit && segment.should_store;
                    if hit {
                        let token_estimate = estimate_message_prefix_tokens(
                            segment.byte_len,
                            full_message_prefix_bytes,
                            raw_input_tokens,
                        );
                        best_message_prefix_hit_tokens =
                            best_message_prefix_hit_tokens.max(token_estimate);
                    }
                }
                SegmentKind::ToolResult => {
                    tool_saw = true;
                    tool_hit |= hit;
                    tool_stored |= !hit;
                    if hit {
                        saved_input_tokens =
                            saved_input_tokens.saturating_add(segment.token_estimate);
                    }
                }
            }
        }

        if best_message_prefix_hit_tokens > 0 {
            saved_input_tokens = saved_input_tokens.saturating_add(best_message_prefix_hit_tokens);
        }

        for segment in segments {
            if !segment.should_store {
                continue;
            }
            if let Err(err) = self.store_segment(&segment, now_secs) {
                tracing::debug!(
                    error = %err,
                    segment = segment.name,
                    "input cache segment write failed"
                );
            }
        }

        let max_savings = ((raw_input_tokens as f64) * MAX_SAVINGS_RATIO).floor() as i64;
        saved_input_tokens = saved_input_tokens.clamp(0, max_savings);
        let estimated_billable_input_tokens =
            raw_input_tokens.saturating_sub(saved_input_tokens).max(0);
        let input_cache_hit_rate = if raw_input_tokens > 0 {
            saved_input_tokens as f64 / raw_input_tokens as f64
        } else {
            0.0
        };

        InputCacheReport {
            raw_input_tokens,
            estimated_billable_input_tokens,
            saved_input_tokens,
            input_cache_hit_rate,
            prefix_cache_state: state_label(prefix_saw, prefix_hit, prefix_stored),
            tool_result_cache_state: state_label(tool_saw, tool_hit, tool_stored),
            input_cache_ttl_secs: observed_ttl_secs,
        }
    }

    fn extract_segments(&self, request: &MessagesRequest) -> Vec<Segment> {
        let mut segments = Vec::new();

        if let Some(system) = request.system.as_ref() {
            if let Ok(value) = serde_json::to_value(system) {
                self.push_segment(
                    &mut segments,
                    SegmentKind::Prefix,
                    "system",
                    self.long_ttl_secs,
                    &value,
                    true,
                );
            }
        }

        if let Some(tools) = request.tools.as_ref() {
            if let Ok(value) = serde_json::to_value(tools) {
                self.push_segment(
                    &mut segments,
                    SegmentKind::Prefix,
                    "tools",
                    self.long_ttl_secs,
                    &value,
                    true,
                );
            }
        }

        if !request.messages.is_empty() {
            let history_prefix = Value::Array(
                request
                    .messages
                    .iter()
                    .filter_map(|message| serde_json::to_value(message).ok())
                    .collect(),
            );
            self.push_segment(
                &mut segments,
                SegmentKind::MessagePrefix,
                "message-prefix",
                self.short_ttl_secs,
                &history_prefix,
                true,
            );

            for end in 1..request.messages.len() {
                let history_prefix = Value::Array(
                    request.messages[..end]
                        .iter()
                        .filter_map(|message| serde_json::to_value(message).ok())
                        .collect(),
                );
                self.push_segment(
                    &mut segments,
                    SegmentKind::MessagePrefix,
                    "message-prefix",
                    self.short_ttl_secs,
                    &history_prefix,
                    false,
                );
            }
        }

        for message in &request.messages {
            collect_tool_result_segments(message, &mut |value| {
                self.push_segment(
                    &mut segments,
                    SegmentKind::ToolResult,
                    "tool-result",
                    self.short_ttl_secs,
                    value,
                    true,
                );
            });
        }

        segments
    }

    fn push_segment(
        &self,
        segments: &mut Vec<Segment>,
        kind: SegmentKind,
        name: &'static str,
        ttl_secs: u64,
        value: &Value,
        should_store: bool,
    ) {
        let Ok(canonical) = serde_json::to_vec(value) else {
            return;
        };
        if canonical.len() < MIN_SEGMENT_CHARS {
            return;
        }

        let mut hasher = Sha256::new();
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(&canonical);
        let key = hex::encode(hasher.finalize());

        segments.push(Segment {
            kind,
            name,
            ttl_secs,
            byte_len: canonical.len(),
            token_estimate: estimate_tokens_from_bytes(canonical.len()),
            key,
            should_store,
        });
    }

    fn segment_is_fresh(&self, segment: &Segment, now_secs: u64) -> bool {
        let path = self.segment_path(&segment.key);
        let Ok(content) = fs::read_to_string(path) else {
            return false;
        };
        let Ok(entry) = serde_json::from_str::<SegmentEntry>(&content) else {
            return false;
        };
        let ttl_secs = entry.ttl_secs.min(segment.ttl_secs);
        now_secs.saturating_sub(entry.cached_at) < ttl_secs
    }

    fn store_segment(&self, segment: &Segment, now_secs: u64) -> std::io::Result<()> {
        let entry = SegmentEntry {
            cached_at: now_secs,
            ttl_secs: segment.ttl_secs,
            token_estimate: segment.token_estimate,
            kind: match segment.kind {
                SegmentKind::Prefix => "prefix",
                SegmentKind::MessagePrefix => "message-prefix",
                SegmentKind::ToolResult => "tool-result",
            }
            .to_string(),
            name: segment.name.to_string(),
        };
        let path = self.segment_path(&segment.key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_vec(&entry)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        fs::write(path, content)
    }

    fn segment_path(&self, key: &str) -> PathBuf {
        let fanout = key.get(0..2).unwrap_or("xx");
        self.root
            .join("input-segments")
            .join(fanout)
            .join(format!("{key}.json"))
    }
}

fn collect_tool_result_segments<'a>(message: &'a Message, visitor: &mut impl FnMut(&'a Value)) {
    if let Value::Array(blocks) = &message.content {
        for block in blocks {
            let is_tool_result = block
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "tool_result");
            if is_tool_result {
                visitor(block);
            }
        }
    }
}

fn state_label(saw: bool, hit: bool, stored: bool) -> String {
    match (saw, hit, stored) {
        (false, _, _) => "bypass",
        (true, true, true) => "partial-hit",
        (true, true, false) => "hit",
        (true, false, true) => "miss-stored",
        (true, false, false) => "miss",
    }
    .to_string()
}

fn estimate_tokens_from_bytes(bytes: usize) -> i64 {
    ((bytes as f64) / 4.0).ceil().max(1.0) as i64
}

fn estimate_message_prefix_tokens(
    prefix_bytes: usize,
    full_message_bytes: usize,
    raw_input_tokens: i64,
) -> i64 {
    if prefix_bytes == 0 || full_message_bytes == 0 || raw_input_tokens <= 0 {
        return estimate_tokens_from_bytes(prefix_bytes);
    }
    let ratio = (prefix_bytes as f64 / full_message_bytes as f64).clamp(0.0, 1.0);
    ((raw_input_tokens as f64) * ratio).floor().max(1.0) as i64
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::{Message, MessagesRequest, SystemMessage, Tool};
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_cache_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kiro-rs-input-cache-{name}-{nanos}"))
    }

    fn sample_request() -> MessagesRequest {
        let mut schema = HashMap::new();
        schema.insert("type".to_string(), json!("object"));
        schema.insert(
            "properties".to_string(),
            json!({"path": {"type": "string"}, "offset": {"type": "integer"}}),
        );

        MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 1024,
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: json!("Here is a long repeated project context. ".repeat(80)),
                },
                Message {
                    role: "assistant".to_string(),
                    content: json!("I have read the project context and will keep it in mind."),
                },
                Message {
                    role: "user".to_string(),
                    content: json!("Now answer the current question without changing the context."),
                },
            ],
            stream: false,
            system: Some(vec![SystemMessage {
                text: "You are a careful coding agent. ".repeat(80),
            }]),
            tools: Some(vec![Tool {
                tool_type: None,
                name: "read_file".to_string(),
                description: "Read a local file for context. ".repeat(40),
                input_schema: schema,
                max_uses: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn repeated_prefix_segments_are_counted_as_input_savings() {
        let dir = temp_cache_dir("repeat");
        let cache = InputCache::new(dir.clone(), 300, 3600);
        let request = sample_request();

        let first = cache.analyze_and_store_at(&request, 10_000, 1_000);
        assert_eq!(first.saved_input_tokens, 0);
        assert_eq!(first.prefix_cache_state, "miss-stored");
        assert_eq!(first.input_cache_ttl_secs, Some(3600));

        let second = cache.analyze_and_store_at(&request, 10_000, 1_060);
        assert!(second.saved_input_tokens > 0);
        assert!(second.input_cache_hit_rate > 0.0);
        assert_eq!(second.prefix_cache_state, "hit");
        assert_eq!(
            second.estimated_billable_input_tokens,
            10_000 - second.saved_input_tokens
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn five_minute_segments_expire_before_one_hour_segments() {
        let dir = temp_cache_dir("ttl");
        let cache = InputCache::new(dir.clone(), 300, 3600);
        let request = sample_request();

        let first = cache.analyze_and_store_at(&request, 10_000, 2_000);
        assert_eq!(first.saved_input_tokens, 0);

        let after_short_ttl = cache.analyze_and_store_at(&request, 10_000, 2_301);
        assert!(after_short_ttl.saved_input_tokens > 0);
        assert_eq!(after_short_ttl.prefix_cache_state, "partial-hit");
        assert_eq!(after_short_ttl.input_cache_ttl_secs, Some(3600));

        let after_long_ttl = cache.analyze_and_store_at(&request, 10_000, 5_901);
        assert_eq!(after_long_ttl.saved_input_tokens, 0);
        assert_eq!(after_long_ttl.prefix_cache_state, "miss-stored");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn appended_conversation_hits_previous_full_message_prefix() {
        let dir = temp_cache_dir("append");
        let cache = InputCache::new(dir.clone(), 300, 3600);

        let first = MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 1024,
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: json!("Shared project context. ".repeat(200)),
                },
                Message {
                    role: "user".to_string(),
                    content: json!("Initial question. ".repeat(40)),
                },
            ],
            stream: true,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        };

        let second = MessagesRequest {
            messages: vec![
                first.messages[0].clone(),
                first.messages[1].clone(),
                Message {
                    role: "assistant".to_string(),
                    content: json!("Initial answer. ".repeat(80)),
                },
                Message {
                    role: "user".to_string(),
                    content: json!("Follow-up question. ".repeat(40)),
                },
            ],
            ..first
        };

        let initial = cache.analyze_and_store_at(&second, 20_000, 10_000);
        assert_eq!(initial.saved_input_tokens, 0);

        let appended = MessagesRequest {
            messages: vec![
                second.messages[0].clone(),
                second.messages[1].clone(),
                second.messages[2].clone(),
                second.messages[3].clone(),
                Message {
                    role: "assistant".to_string(),
                    content: json!("Follow-up answer. ".repeat(80)),
                },
                Message {
                    role: "user".to_string(),
                    content: json!("Second follow-up. ".repeat(40)),
                },
            ],
            ..second
        };

        let report = cache.analyze_and_store_at(&appended, 20_000, 10_060);
        assert!(report.saved_input_tokens > 0);
        assert!(report.input_cache_hit_rate > 0.0);
        assert_eq!(report.prefix_cache_state, "partial-hit");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn appended_dominant_prefix_reaches_savings_cap() {
        let dir = temp_cache_dir("append-cap");
        let cache = InputCache::new(dir.clone(), 300, 3600);
        let reusable_message = Message {
            role: "user".to_string(),
            content: json!("Long reusable Chinese project context. ".repeat(2000)),
        };

        let first = MessagesRequest {
            model: "claude-haiku-4-5-20251001".to_string(),
            max_tokens: 1,
            messages: vec![reusable_message.clone()],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        };

        let second = MessagesRequest {
            model: "claude-haiku-4-5-20251001".to_string(),
            max_tokens: 1,
            messages: vec![
                reusable_message,
                Message {
                    role: "assistant".to_string(),
                    content: json!("OK"),
                },
                Message {
                    role: "user".to_string(),
                    content: json!("Short follow-up."),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        };

        let initial = cache.analyze_and_store_at(&first, 100_000, 20_000);
        assert_eq!(initial.saved_input_tokens, 0);

        let report = cache.analyze_and_store_at(&second, 100_000, 20_060);
        assert_eq!(report.saved_input_tokens, 90_000);
        assert_eq!(report.input_cache_hit_rate, 0.9);
        assert_eq!(report.prefix_cache_state, "partial-hit");

        let _ = std::fs::remove_dir_all(dir);
    }
}
