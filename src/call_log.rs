//! Lightweight call log storage for Admin API inspection.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct CallLogStore {
    path: PathBuf,
    max_records: usize,
    max_body_bytes: usize,
    lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallLogRecord {
    pub id: String,
    pub created_at: String,
    pub endpoint: String,
    pub model: String,
    pub stream: bool,
    pub status: String,
    pub http_status: u16,
    pub cache_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_billable_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cache_hit_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_cache_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_cache_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cache_ttl_secs: Option<u64>,
    pub duration_ms: u64,
    pub request_bytes: usize,
    pub response_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallLogQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cache_state: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

impl Default for CallLogQuery {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            offset: 0,
            model: None,
            cache_state: None,
            status: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallLogListResponse {
    pub enabled: bool,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub records: Vec<CallLogRecord>,
}

fn default_limit() -> usize {
    50
}

impl CallLogStore {
    pub fn new(dir: PathBuf, max_records: usize, max_body_bytes: usize) -> Self {
        Self {
            path: dir.join("call_logs.jsonl"),
            max_records: max_records.max(1),
            max_body_bytes,
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn append(&self, mut record: CallLogRecord) -> std::io::Result<()> {
        let _guard = self.lock.lock();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        record.request = record
            .request
            .take()
            .map(|value| truncate_json(value, self.max_body_bytes).0);
        record.response = record
            .response
            .take()
            .map(|value| truncate_json(value, self.max_body_bytes).0);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        self.compact_locked()
    }

    pub fn list(&self, mut query: CallLogQuery) -> CallLogListResponse {
        query.limit = query.limit.clamp(1, 200);

        let records = self.read_all();
        let mut filtered: Vec<CallLogRecord> = records
            .into_iter()
            .rev()
            .filter(|record| match query.model.as_deref() {
                Some(model) if !model.trim().is_empty() => record.model.contains(model),
                _ => true,
            })
            .filter(|record| match query.cache_state.as_deref() {
                Some(cache_state) if !cache_state.trim().is_empty() => {
                    record.cache_state == cache_state
                }
                _ => true,
            })
            .filter(|record| match query.status.as_deref() {
                Some(status) if !status.trim().is_empty() => record.status == status,
                _ => true,
            })
            .collect();

        let total = filtered.len();
        let records = filtered
            .drain(query.offset.min(total)..)
            .take(query.limit)
            .collect();

        CallLogListResponse {
            enabled: true,
            total,
            limit: query.limit,
            offset: query.offset,
            records,
        }
    }

    fn read_all(&self) -> Vec<CallLogRecord> {
        let _guard = self.lock.lock();
        read_records(&self.path)
    }

    fn compact_locked(&self) -> std::io::Result<()> {
        let records = read_records(&self.path);
        if records.len() <= self.max_records {
            return Ok(());
        }

        let keep_from = records.len().saturating_sub(self.max_records);
        let tmp_path = self.path.with_extension("jsonl.tmp");
        let mut tmp = File::create(&tmp_path)?;
        for record in records.into_iter().skip(keep_from) {
            serde_json::to_writer(&mut tmp, &record)?;
            tmp.write_all(b"\n")?;
        }
        fs::rename(tmp_path, &self.path)
    }
}

pub fn disabled_call_logs(query: CallLogQuery) -> CallLogListResponse {
    CallLogListResponse {
        enabled: false,
        total: 0,
        limit: query.limit.clamp(1, 200),
        offset: query.offset,
        records: Vec::new(),
    }
}

pub fn json_size(value: &Value) -> usize {
    serde_json::to_vec(value).map(|v| v.len()).unwrap_or(0)
}

fn truncate_json(value: Value, max_bytes: usize) -> (Value, usize) {
    let bytes = json_size(&value);
    if max_bytes == 0 || bytes <= max_bytes {
        return (value, bytes);
    }

    (
        json!({
            "truncated": true,
            "bytes": bytes,
            "maxBytes": max_bytes
        }),
        bytes,
    )
}

fn read_records(path: &PathBuf) -> Vec<CallLogRecord> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };

    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<CallLogRecord>(&line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(id: &str, model: &str, response: Value) -> CallLogRecord {
        CallLogRecord {
            id: id.to_string(),
            created_at: "2026-05-09T00:00:00Z".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: model.to_string(),
            stream: false,
            status: "success".to_string(),
            http_status: 200,
            cache_state: "bypass".to_string(),
            cache_key: None,
            credential_id: Some(1),
            input_tokens: Some(10),
            output_tokens: Some(2),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            raw_input_tokens: None,
            estimated_billable_input_tokens: None,
            saved_input_tokens: None,
            input_cache_hit_rate: None,
            prefix_cache_state: None,
            tool_result_cache_state: None,
            input_cache_ttl_secs: None,
            duration_ms: 12,
            request_bytes: 0,
            response_bytes: 0,
            request: Some(json!({ "model": model })),
            response: Some(response),
            error: None,
        }
    }

    fn temp_store(max_records: usize, max_body_bytes: usize) -> CallLogStore {
        let dir = std::env::temp_dir().join(format!("kiro-call-log-test-{}", uuid::Uuid::new_v4()));
        CallLogStore::new(dir, max_records, max_body_bytes)
    }

    #[test]
    fn list_returns_newest_first_and_compacts() {
        let store = temp_store(2, 1024);
        store
            .append(sample_record("one", "claude-a", json!({ "text": "1" })))
            .unwrap();
        store
            .append(sample_record("two", "claude-b", json!({ "text": "2" })))
            .unwrap();
        store
            .append(sample_record("three", "claude-b", json!({ "text": "3" })))
            .unwrap();

        let list = store.list(CallLogQuery::default());

        assert_eq!(list.total, 2);
        assert_eq!(list.records[0].id, "three");
        assert_eq!(list.records[1].id, "two");
    }

    #[test]
    fn append_truncates_large_json_bodies() {
        let store = temp_store(10, 16);
        store
            .append(sample_record(
                "one",
                "claude-a",
                json!({ "text": "this body should be truncated" }),
            ))
            .unwrap();

        let list = store.list(CallLogQuery::default());
        let response = list.records[0].response.as_ref().unwrap();

        assert_eq!(
            response.get("truncated").and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}
