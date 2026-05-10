//! True cache support for Anthropic-compatible requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
struct CachedResponse {
    cached_at: u64,
    value: Value,
}

pub struct TrueCache {
    root: PathBuf,
    response_ttl: Duration,
    max_response_bytes: usize,
}

impl TrueCache {
    pub fn new(root: PathBuf, response_ttl: Duration, max_response_bytes: usize) -> Self {
        Self {
            root,
            response_ttl,
            max_response_bytes,
        }
    }

    pub fn response_key(_request_body: &str) -> Option<String> {
        let mut value: Value = serde_json::from_str(_request_body).ok()?;
        normalize_request_value(&mut value)?;

        let canonical = serde_json::to_vec(&value).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        Some(hex::encode(hasher.finalize()))
    }

    pub fn get_response(&self, _key: &str) -> Option<Value> {
        let path = self.response_path(_key);
        let metadata = fs::metadata(&path).ok()?;
        if metadata.len() as usize > self.max_response_bytes {
            let _ = fs::remove_file(path);
            return None;
        }

        let content = fs::read_to_string(&path).ok()?;
        let cached: CachedResponse = serde_json::from_str(&content).ok()?;
        if self.is_expired(cached.cached_at) {
            let _ = fs::remove_file(path);
            return None;
        }

        Some(cached.value)
    }

    pub fn put_response(&self, _key: &str, _value: &Value) -> std::io::Result<()> {
        let body = CachedResponse {
            cached_at: now_unix_secs(),
            value: _value.clone(),
        };
        let content = serde_json::to_vec(&body)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        if content.len() > self.max_response_bytes {
            return Ok(());
        }

        let path = self.response_path(_key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)
    }

    fn response_path(&self, key: &str) -> PathBuf {
        let fanout = key.get(0..2).unwrap_or("xx");
        self.root
            .join("responses")
            .join(fanout)
            .join(format!("{key}.json"))
    }

    fn is_expired(&self, cached_at: u64) -> bool {
        let now = now_unix_secs();
        now.saturating_sub(cached_at) >= self.response_ttl.as_secs()
    }
}

fn normalize_request_value(value: &mut Value) -> Option<()> {
    let state = value.get_mut("conversationState")?.as_object_mut()?;
    state.remove("agentContinuationId");
    state.remove("conversationId");
    let mut normalizer = ToolUseIdNormalizer::default();
    normalizer.normalize(value);
    Some(())
}

#[derive(Default)]
struct ToolUseIdNormalizer {
    ids: HashMap<String, String>,
}

impl ToolUseIdNormalizer {
    fn normalize(&mut self, value: &mut Value) {
        match value {
            Value::Array(items) => {
                for item in items {
                    self.normalize(item);
                }
            }
            Value::Object(map) => {
                let is_tool_use_object = map
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == "tool_use");

                for (key, item) in map {
                    if matches!(key.as_str(), "toolUseId" | "tool_use_id")
                        || (is_tool_use_object && key == "id")
                    {
                        if let Some(id) = item.as_str() {
                            *item = Value::String(self.normalized_id(id));
                        }
                    } else {
                        self.normalize(item);
                    }
                }
            }
            _ => {}
        }
    }

    fn normalized_id(&mut self, id: &str) -> String {
        if let Some(mapped) = self.ids.get(id) {
            return mapped.clone();
        }

        let mapped = format!("tooluse_{:04}", self.ids.len() + 1);
        self.ids.insert(id.to_string(), mapped.clone());
        mapped
    }
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_cache_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kiro-rs-true-cache-{name}-{nanos}"))
    }

    #[test]
    fn response_key_ignores_kiro_volatile_ids() {
        let left = r#"{
            "conversationState": {
                "agentContinuationId": "11111111-1111-1111-1111-111111111111",
                "conversationId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "currentMessage": {
                    "userInputMessage": {
                        "content": "hello",
                        "modelId": "claude-sonnet-4.6",
                        "userInputMessageContext": {}
                    }
                }
            }
        }"#;
        let right = r#"{
            "conversationState": {
                "agentContinuationId": "22222222-2222-2222-2222-222222222222",
                "conversationId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                "currentMessage": {
                    "userInputMessage": {
                        "content": "hello",
                        "modelId": "claude-sonnet-4.6",
                        "userInputMessageContext": {}
                    }
                }
            }
        }"#;

        let left_key = TrueCache::response_key(left);
        assert!(left_key.is_some());
        assert_eq!(left_key, TrueCache::response_key(right));
    }

    #[test]
    fn response_key_normalizes_tool_use_ids() {
        let left = r#"{
            "conversationState": {
                "conversationId": "conv-a",
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6",
                        "userInputMessageContext": {
                            "toolResults": [{
                                "toolUseId": "toolu_random_a",
                                "content": [{"text": "same result"}],
                                "status": "success"
                            }]
                        }
                    }
                },
                "history": [{
                    "assistantResponseMessage": {
                        "content": "",
                        "toolUses": [{
                            "toolUseId": "toolu_random_a",
                            "name": "read_file",
                            "input": {"path": "/tmp/a.txt"}
                        }]
                    }
                }]
            }
        }"#;
        let right = r#"{
            "conversationState": {
                "conversationId": "conv-b",
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6",
                        "userInputMessageContext": {
                            "toolResults": [{
                                "toolUseId": "toolu_random_b",
                                "content": [{"text": "same result"}],
                                "status": "success"
                            }]
                        }
                    }
                },
                "history": [{
                    "assistantResponseMessage": {
                        "content": "",
                        "toolUses": [{
                            "toolUseId": "toolu_random_b",
                            "name": "read_file",
                            "input": {"path": "/tmp/a.txt"}
                        }]
                    }
                }]
            }
        }"#;

        assert_eq!(
            TrueCache::response_key(left),
            TrueCache::response_key(right)
        );
    }

    #[test]
    fn response_key_normalizes_anthropic_tool_use_ids() {
        let left = r#"{
            "conversationState": {
                "history": [{
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_random_a",
                        "name": "read_file",
                        "input": {"path": "/tmp/a.txt"}
                    }]
                }, {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_random_a",
                        "content": "same result"
                    }]
                }],
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6"
                    }
                }
            }
        }"#;
        let right = r#"{
            "conversationState": {
                "history": [{
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_random_b",
                        "name": "read_file",
                        "input": {"path": "/tmp/a.txt"}
                    }]
                }, {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_random_b",
                        "content": "same result"
                    }]
                }],
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6"
                    }
                }
            }
        }"#;

        assert_eq!(
            TrueCache::response_key(left),
            TrueCache::response_key(right)
        );
    }

    #[test]
    fn response_key_keeps_non_tool_ids_significant() {
        let left = r#"{
            "conversationState": {
                "history": [{
                    "role": "assistant",
                    "content": [{"type": "text", "id": "text-a", "text": "same text"}]
                }],
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6"
                    }
                }
            }
        }"#;
        let right = r#"{
            "conversationState": {
                "history": [{
                    "role": "assistant",
                    "content": [{"type": "text", "id": "text-b", "text": "same text"}]
                }],
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6"
                    }
                }
            }
        }"#;

        assert_ne!(
            TrueCache::response_key(left),
            TrueCache::response_key(right)
        );
    }

    #[test]
    fn response_key_keeps_tool_result_content_significant() {
        let left = r#"{
            "conversationState": {
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6",
                        "userInputMessageContext": {
                            "toolResults": [{
                                "toolUseId": "toolu_random_a",
                                "content": [{"text": "left result"}]
                            }]
                        }
                    }
                }
            }
        }"#;
        let right = r#"{
            "conversationState": {
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-sonnet-4.6",
                        "userInputMessageContext": {
                            "toolResults": [{
                                "toolUseId": "toolu_random_b",
                                "content": [{"text": "right result"}]
                            }]
                        }
                    }
                }
            }
        }"#;

        assert_ne!(
            TrueCache::response_key(left),
            TrueCache::response_key(right)
        );
    }

    #[test]
    fn response_cache_round_trips_json_to_disk() {
        let dir = temp_cache_dir("roundtrip");
        let cache = TrueCache::new(dir.clone(), Duration::from_secs(60), 1024 * 1024);
        let key = TrueCache::response_key(r#"{"conversationState":{"currentMessage":{"userInputMessage":{"content":"ping","modelId":"claude-sonnet-4.6"}}}}"#)
            .expect("cache key");
        let value = serde_json::json!({
            "type": "message",
            "content": [{"type": "text", "text": "pong"}],
            "usage": {"input_tokens": 12, "output_tokens": 3}
        });

        cache.put_response(&key, &value).expect("write cache");
        assert_eq!(cache.get_response(&key), Some(value));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn response_cache_respects_ttl() {
        let dir = temp_cache_dir("ttl");
        let cache = TrueCache::new(dir.clone(), Duration::from_secs(0), 1024 * 1024);
        let key = TrueCache::response_key(r#"{"conversationState":{"currentMessage":{"userInputMessage":{"content":"stale","modelId":"claude-sonnet-4.6"}}}}"#)
            .expect("cache key");

        cache
            .put_response(&key, &serde_json::json!({"type": "message"}))
            .expect("write cache");

        assert_eq!(cache.get_response(&key), None);

        let _ = std::fs::remove_dir_all(dir);
    }
}
