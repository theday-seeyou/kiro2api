//! Anthropic API Handler 函数

use std::convert::Infallible;
use std::sync::Arc;

use crate::call_log::{CallLogRecord, CallLogStore, json_size};
use crate::kiro::model::events::Event;
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::token;
use anyhow::Error;
use axum::{
    Json as JsonExtractor,
    body::Body,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::time::interval;
use uuid::Uuid;

use super::converter::{ConversionError, convert_request};
use super::middleware::AppState;
use super::stream::{BufferedStreamContext, SseEvent, StreamContext};
use super::types::{
    CountTokensRequest, CountTokensResponse, ErrorResponse, MessagesRequest, Model, ModelsResponse,
    OutputConfig, Thinking,
};
use super::websearch;
use super::{InputCacheReport, TrueCache};

/// 将 KiroProvider 错误映射为 HTTP 响应
fn map_provider_error(err: Error) -> Response {
    let err_str = err.to_string();

    // 上下文窗口满了（对话历史累积超出模型上下文窗口限制）
    if err_str.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") {
        tracing::warn!(error = %err, "上游拒绝请求：上下文窗口已满（不应重试）");
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new(
                "invalid_request_error",
                "Context window is full. Reduce conversation history, system prompt, or tools.",
            )),
        )
            .into_response();
    }

    // 单次输入太长（请求体本身超出上游限制）
    if err_str.contains("Input is too long") {
        tracing::warn!(error = %err, "上游拒绝请求：输入过长（不应重试）");
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new(
                "invalid_request_error",
                "Input is too long. Reduce the size of your messages.",
            )),
        )
            .into_response();
    }
    tracing::error!("Kiro API 调用失败: {}", err);
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse::new(
            "api_error",
            format!("上游 API 调用失败: {}", err),
        )),
    )
        .into_response()
}

fn is_true_cache_eligible(payload: &MessagesRequest) -> bool {
    tool_choice_allows_response_cache(payload.tool_choice.as_ref())
}

fn tool_choice_allows_response_cache(tool_choice: Option<&serde_json::Value>) -> bool {
    match tool_choice {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::Object(choice)) => choice
            .get("type")
            .and_then(|value| value.as_str())
            .is_some_and(|choice_type| matches!(choice_type, "auto" | "none")),
        Some(serde_json::Value::String(choice_type)) => {
            matches!(choice_type.as_str(), "auto" | "none")
        }
        _ => false,
    }
}

fn response_cache_key(payload: &MessagesRequest, request_body: &str) -> Option<String> {
    TrueCache::response_key(request_body).map(|key| {
        if payload.stream {
            format!("stream-{key}")
        } else {
            key
        }
    })
}

fn sse_events_to_cache_value(events: &[SseEvent]) -> serde_json::Value {
    json!({
        "kind": "sse",
        "events": events
            .iter()
            .map(|event| json!({ "event": event.event, "data": event.data.clone() }))
            .collect::<Vec<_>>()
    })
}

fn sse_events_from_cache_value(value: &serde_json::Value) -> Option<Vec<SseEvent>> {
    if value.get("kind").and_then(|v| v.as_str()) != Some("sse") {
        return None;
    }

    value
        .get("events")?
        .as_array()?
        .iter()
        .map(|event| {
            Some(SseEvent::new(
                event.get("event")?.as_str()?.to_string(),
                event.get("data")?.clone(),
            ))
        })
        .collect()
}

fn sse_events_are_cacheable(events: &[SseEvent]) -> bool {
    events
        .iter()
        .rev()
        .find(|event| event.event == "message_delta")
        .and_then(|event| event.data.get("delta"))
        .and_then(|delta| delta.get("stop_reason"))
        .and_then(|value| value.as_str())
        .is_some_and(|stop_reason| stop_reason == "end_turn" || stop_reason == "tool_use")
}

fn mark_cache_hit_usage(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(id) = value.get_mut("id") {
        *id = json!(format!("msg_{}", Uuid::new_v4().simple()));
    }

    if let Some(usage) = value.get_mut("usage").and_then(|v| v.as_object_mut()) {
        let input_tokens = usage_input_token_total(usage);
        usage.insert(
            "input_tokens".to_string(),
            serde_json::Value::Number(0.into()),
        );
        usage.insert(
            "cache_read_input_tokens".to_string(),
            serde_json::Value::Number(input_tokens.into()),
        );
        usage.insert(
            "cache_creation_input_tokens".to_string(),
            serde_json::Value::Number(0.into()),
        );
    }

    refresh_tool_use_ids_in_json(&mut value);
    value
}

fn refresh_tool_use_ids_in_json(value: &mut serde_json::Value) {
    if let Some(content) = value.get_mut("content").and_then(|v| v.as_array_mut()) {
        for block in content {
            let is_tool_use = block
                .get("type")
                .and_then(|v| v.as_str())
                .is_some_and(|block_type| block_type == "tool_use");
            if is_tool_use && let Some(id) = block.get_mut("id") {
                *id = json!(format!("tooluse_{}", Uuid::new_v4().simple()));
            }
        }
    }
}

fn usage_input_token_total(usage: &serde_json::Map<String, serde_json::Value>) -> i64 {
    positive(usage.get("input_tokens").and_then(|value| value.as_i64()))
        .saturating_add(positive(
            usage
                .get("cache_read_input_tokens")
                .and_then(|value| value.as_i64()),
        ))
        .saturating_add(positive(
            usage
                .get("cache_creation_input_tokens")
                .and_then(|value| value.as_i64()),
        ))
}

fn usage_input_token_total_from_value(value: &serde_json::Value) -> Option<i64> {
    let usage = value.as_object()?;
    Some(usage_input_token_total(usage))
}

fn mark_cache_hit_sse_events(mut events: Vec<SseEvent>) -> Vec<SseEvent> {
    let cache_read_tokens = events
        .iter()
        .filter_map(|event| {
            if event.event == "message_start" {
                event
                    .data
                    .get("message")?
                    .get("usage")
                    .and_then(usage_input_token_total_from_value)
            } else if event.event == "message_delta" {
                event
                    .data
                    .get("usage")
                    .and_then(usage_input_token_total_from_value)
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);

    for event in &mut events {
        if event.event == "message_start" {
            if let Some(message) = event
                .data
                .get_mut("message")
                .and_then(|v| v.as_object_mut())
            {
                message.insert(
                    "id".to_string(),
                    json!(format!("msg_{}", Uuid::new_v4().simple())),
                );

                if let Some(usage) = message.get_mut("usage").and_then(|v| v.as_object_mut()) {
                    usage.insert(
                        "input_tokens".to_string(),
                        serde_json::Value::Number(0.into()),
                    );
                    usage.insert(
                        "cache_read_input_tokens".to_string(),
                        serde_json::Value::Number(cache_read_tokens.into()),
                    );
                    usage.insert(
                        "cache_creation_input_tokens".to_string(),
                        serde_json::Value::Number(0.into()),
                    );
                }
            }
        } else if event.event == "content_block_start"
            && let Some(content_block) = event
                .data
                .get_mut("content_block")
                .and_then(|v| v.as_object_mut())
        {
            let is_tool_use = content_block
                .get("type")
                .and_then(|v| v.as_str())
                .is_some_and(|block_type| block_type == "tool_use");
            if is_tool_use {
                content_block.insert(
                    "id".to_string(),
                    json!(format!("tooluse_{}", Uuid::new_v4().simple())),
                );
            }
        } else if event.event == "message_delta"
            && let Some(usage) = event.data.get_mut("usage").and_then(|v| v.as_object_mut())
        {
            usage.insert(
                "input_tokens".to_string(),
                serde_json::Value::Number(0.into()),
            );
            usage.insert(
                "cache_read_input_tokens".to_string(),
                serde_json::Value::Number(cache_read_tokens.into()),
            );
            usage.insert(
                "cache_creation_input_tokens".to_string(),
                serde_json::Value::Number(0.into()),
            );
        }
    }

    events
}

fn ok_json_with_cache_header(value: serde_json::Value, cache_state: &'static str) -> Response {
    let mut response = (StatusCode::OK, Json(value)).into_response();
    response
        .headers_mut()
        .insert("x-kiro-true-cache", HeaderValue::from_static(cache_state));
    response
}

fn ok_sse_with_cache_header(events: Vec<SseEvent>, cache_state: &'static str) -> Response {
    let stream = stream::iter(
        events
            .into_iter()
            .map(|event| Ok::<Bytes, Infallible>(Bytes::from(event.to_sse_string()))),
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header("x-kiro-true-cache", cache_state)
        .body(Body::from_stream(stream))
        .unwrap()
}

#[derive(Clone)]
struct CallLogContext {
    store: Option<Arc<CallLogStore>>,
    started_at: Instant,
    endpoint: &'static str,
    model: String,
    stream: bool,
    request: Option<serde_json::Value>,
    request_bytes: usize,
    cache_key: Option<String>,
    input_cache_report: Option<InputCacheReport>,
}

#[derive(Default)]
struct UsageParts {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
}

#[derive(Debug, PartialEq)]
struct InputCacheAccounting {
    raw_input_tokens: Option<i64>,
    estimated_billable_input_tokens: Option<i64>,
    saved_input_tokens: Option<i64>,
    input_cache_hit_rate: Option<f64>,
}

#[derive(Debug, PartialEq)]
struct AdjustedInputCacheUsage {
    billable_input_tokens: i64,
    cache_read_input_tokens: i64,
}

fn positive(value: Option<i64>) -> i64 {
    value.unwrap_or(0).max(0)
}

fn compute_input_cache_accounting(
    input_cache: Option<&InputCacheReport>,
    usage: &UsageParts,
) -> InputCacheAccounting {
    let local_raw_input_tokens = input_cache.map(|report| report.raw_input_tokens.max(0));
    let upstream_total_input_tokens = positive(usage.input_tokens)
        .saturating_add(positive(usage.cache_read_input_tokens))
        .saturating_add(positive(usage.cache_creation_input_tokens));

    let raw_input_tokens = match (local_raw_input_tokens, upstream_total_input_tokens) {
        (Some(local), upstream) if upstream > 0 => Some(local.max(upstream)),
        (Some(local), _) => Some(local),
        (None, upstream) if upstream > 0 => Some(upstream),
        (None, _) => None,
    };

    let estimated_saved_input_tokens = input_cache
        .map(|report| report.saved_input_tokens.max(0))
        .unwrap_or(0);
    let scaled_estimated_saved_input_tokens = input_cache
        .and_then(|report| {
            raw_input_tokens.map(|raw| {
                let hit_rate = report.input_cache_hit_rate.clamp(0.0, 1.0);
                ((raw as f64) * hit_rate).floor() as i64
            })
        })
        .unwrap_or(0);
    let upstream_saved_input_tokens = positive(usage.cache_read_input_tokens);
    let uncapped_saved_input_tokens = estimated_saved_input_tokens
        .max(scaled_estimated_saved_input_tokens)
        .max(upstream_saved_input_tokens);
    let saved_input_tokens = raw_input_tokens
        .map(|raw| uncapped_saved_input_tokens.min(raw))
        .unwrap_or(uncapped_saved_input_tokens);
    let estimated_billable_input_tokens =
        raw_input_tokens.map(|raw| raw.saturating_sub(saved_input_tokens));
    let input_cache_hit_rate = raw_input_tokens
        .filter(|raw| *raw > 0)
        .map(|raw| (saved_input_tokens as f64 / raw as f64).clamp(0.0, 1.0));

    InputCacheAccounting {
        raw_input_tokens,
        estimated_billable_input_tokens,
        saved_input_tokens: if saved_input_tokens > 0 {
            Some(saved_input_tokens)
        } else {
            input_cache.map(|report| report.saved_input_tokens.max(0))
        },
        input_cache_hit_rate: input_cache_hit_rate
            .or_else(|| input_cache.map(|report| report.input_cache_hit_rate.clamp(0.0, 1.0))),
    }
}

fn adjusted_input_cache_usage(
    input_cache: Option<&InputCacheReport>,
    usage: &UsageParts,
) -> Option<AdjustedInputCacheUsage> {
    input_cache?;
    let accounting = compute_input_cache_accounting(input_cache, usage);
    let raw_input_tokens = accounting.raw_input_tokens?;
    let saved_input_tokens = accounting
        .saved_input_tokens
        .unwrap_or(0)
        .clamp(0, raw_input_tokens);
    if raw_input_tokens <= 0 || saved_input_tokens <= 0 {
        return None;
    }

    Some(AdjustedInputCacheUsage {
        billable_input_tokens: accounting
            .estimated_billable_input_tokens
            .unwrap_or_else(|| raw_input_tokens.saturating_sub(saved_input_tokens))
            .max(0),
        cache_read_input_tokens: saved_input_tokens,
    })
}

fn write_adjusted_input_cache_usage(
    usage: &mut serde_json::Map<String, serde_json::Value>,
    adjusted: &AdjustedInputCacheUsage,
) {
    usage.insert(
        "input_tokens".to_string(),
        serde_json::Value::Number(adjusted.billable_input_tokens.into()),
    );
    usage.insert(
        "cache_read_input_tokens".to_string(),
        serde_json::Value::Number(adjusted.cache_read_input_tokens.into()),
    );
    usage.insert(
        "cache_creation_input_tokens".to_string(),
        serde_json::Value::Number(0.into()),
    );
}

fn apply_input_cache_usage_to_json(
    value: &mut serde_json::Value,
    input_cache: Option<&InputCacheReport>,
) -> bool {
    let Some(adjusted) = adjusted_input_cache_usage(input_cache, &usage_from_json(value)) else {
        return false;
    };
    let Some(usage) = value
        .get_mut("usage")
        .and_then(|value| value.as_object_mut())
    else {
        return false;
    };
    write_adjusted_input_cache_usage(usage, &adjusted);
    true
}

fn apply_input_cache_usage_to_sse_events(
    events: &mut [SseEvent],
    input_cache: Option<&InputCacheReport>,
) -> bool {
    let Some(adjusted) = adjusted_input_cache_usage(input_cache, &usage_from_sse_events(events))
    else {
        return false;
    };

    let mut applied = false;
    for event in events {
        let usage = if event.event == "message_start" {
            event
                .data
                .get_mut("message")
                .and_then(|value| value.get_mut("usage"))
                .and_then(|value| value.as_object_mut())
        } else if event.event == "message_delta" {
            event
                .data
                .get_mut("usage")
                .and_then(|value| value.as_object_mut())
        } else {
            None
        };

        if let Some(usage) = usage {
            write_adjusted_input_cache_usage(usage, &adjusted);
            applied = true;
        }
    }

    applied
}

fn sse_events_have_visible_text(events: &[SseEvent]) -> bool {
    events.iter().any(|event| {
        event.event == "content_block_delta"
            && event
                .data
                .get("delta")
                .and_then(|delta| delta.get("type"))
                .and_then(|value| value.as_str())
                .is_some_and(|delta_type| delta_type == "text_delta")
            && event
                .data
                .get("delta")
                .and_then(|delta| delta.get("text"))
                .and_then(|value| value.as_str())
                .is_some_and(|text| !text.trim().is_empty())
    })
}

fn sse_events_have_tool_use(events: &[SseEvent]) -> bool {
    events.iter().any(|event| {
        event.event == "content_block_start"
            && event
                .data
                .get("content_block")
                .and_then(|block| block.get("type"))
                .and_then(|value| value.as_str())
                .is_some_and(|block_type| block_type == "tool_use")
    })
}

fn json_response_has_visible_text(value: &serde_json::Value) -> bool {
    value
        .get("content")
        .and_then(|content| content.as_array())
        .is_some_and(|content| {
            content.iter().any(|block| {
                block
                    .get("type")
                    .and_then(|value| value.as_str())
                    .is_some_and(|block_type| block_type == "text")
                    && block
                        .get("text")
                        .and_then(|value| value.as_str())
                        .is_some_and(|text| !text.trim().is_empty())
            })
        })
}

fn json_response_has_tool_use(value: &serde_json::Value) -> bool {
    value
        .get("content")
        .and_then(|content| content.as_array())
        .is_some_and(|content| {
            content.iter().any(|block| {
                block
                    .get("type")
                    .and_then(|value| value.as_str())
                    .is_some_and(|block_type| block_type == "tool_use")
            })
        })
}

fn zero_usage_fields(usage: &mut serde_json::Map<String, serde_json::Value>) {
    usage.insert(
        "input_tokens".to_string(),
        serde_json::Value::Number(0.into()),
    );
    usage.insert(
        "output_tokens".to_string(),
        serde_json::Value::Number(0.into()),
    );
    usage.insert(
        "cache_read_input_tokens".to_string(),
        serde_json::Value::Number(0.into()),
    );
    usage.insert(
        "cache_creation_input_tokens".to_string(),
        serde_json::Value::Number(0.into()),
    );
}

fn zero_empty_visible_sse_usage(history: &[SseEvent], final_events: &mut [SseEvent]) -> bool {
    if sse_events_have_visible_text(history)
        || sse_events_have_visible_text(final_events)
        || sse_events_have_tool_use(history)
        || sse_events_have_tool_use(final_events)
    {
        return false;
    }

    let mut applied = false;
    for event in final_events {
        let usage = if event.event == "message_start" {
            event
                .data
                .get_mut("message")
                .and_then(|value| value.get_mut("usage"))
                .and_then(|value| value.as_object_mut())
        } else if event.event == "message_delta" {
            event
                .data
                .get_mut("usage")
                .and_then(|value| value.as_object_mut())
        } else {
            None
        };

        if let Some(usage) = usage {
            zero_usage_fields(usage);
            applied = true;
        }
    }

    applied
}

fn zero_stream_message_start_usage(events: &mut [SseEvent]) -> bool {
    let mut applied = false;
    for event in events {
        if event.event != "message_start" {
            continue;
        }

        let Some(usage) = event
            .data
            .get_mut("message")
            .and_then(|value| value.get_mut("usage"))
            .and_then(|value| value.as_object_mut())
        else {
            continue;
        };

        zero_usage_fields(usage);
        applied = true;
    }

    applied
}

fn zero_empty_visible_sse_usage_in_events(events: &mut [SseEvent]) -> bool {
    if sse_events_have_visible_text(events) || sse_events_have_tool_use(events) {
        return false;
    }

    let mut applied = false;
    for event in events {
        let usage = if event.event == "message_start" {
            event
                .data
                .get_mut("message")
                .and_then(|value| value.get_mut("usage"))
                .and_then(|value| value.as_object_mut())
        } else if event.event == "message_delta" {
            event
                .data
                .get_mut("usage")
                .and_then(|value| value.as_object_mut())
        } else {
            None
        };

        if let Some(usage) = usage {
            zero_usage_fields(usage);
            applied = true;
        }
    }

    applied
}

fn zero_empty_visible_json_usage(value: &mut serde_json::Value) -> bool {
    if json_response_has_visible_text(value) || json_response_has_tool_use(value) {
        return false;
    }

    let Some(usage) = value
        .get_mut("usage")
        .and_then(|value| value.as_object_mut())
    else {
        return false;
    };
    zero_usage_fields(usage);
    true
}

impl CallLogContext {
    fn new(
        store: Option<Arc<CallLogStore>>,
        endpoint: &'static str,
        model: &str,
        stream: bool,
        request: Option<serde_json::Value>,
        cache_key: Option<String>,
        input_cache_report: Option<InputCacheReport>,
    ) -> Self {
        let request_bytes = request.as_ref().map(json_size).unwrap_or(0);
        Self {
            store,
            started_at: Instant::now(),
            endpoint,
            model: model.to_string(),
            stream,
            request,
            request_bytes,
            cache_key,
            input_cache_report,
        }
    }

    fn success(
        &self,
        http_status: u16,
        cache_state: impl Into<String>,
        credential_id: Option<u64>,
        response: Option<serde_json::Value>,
        usage: UsageParts,
    ) {
        self.append(
            "success",
            http_status,
            cache_state,
            credential_id,
            response,
            None,
            usage,
        );
    }

    fn error(
        &self,
        http_status: u16,
        cache_state: impl Into<String>,
        credential_id: Option<u64>,
        error: String,
        response: Option<serde_json::Value>,
    ) {
        self.append(
            "error",
            http_status,
            cache_state,
            credential_id,
            response,
            Some(error),
            UsageParts::default(),
        );
    }

    fn append(
        &self,
        status: &str,
        http_status: u16,
        cache_state: impl Into<String>,
        credential_id: Option<u64>,
        response: Option<serde_json::Value>,
        error: Option<String>,
        usage: UsageParts,
    ) {
        let Some(store) = &self.store else {
            return;
        };

        let response_bytes = response.as_ref().map(json_size).unwrap_or(0);
        let input_cache = self.input_cache_report.clone();
        let accounting = compute_input_cache_accounting(input_cache.as_ref(), &usage);
        let record = CallLogRecord {
            id: format!("call_{}", Uuid::new_v4().simple()),
            created_at: chrono::Utc::now().to_rfc3339(),
            endpoint: self.endpoint.to_string(),
            model: self.model.clone(),
            stream: self.stream,
            status: status.to_string(),
            http_status,
            cache_state: cache_state.into(),
            cache_key: self.cache_key.clone(),
            credential_id,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            raw_input_tokens: accounting.raw_input_tokens,
            estimated_billable_input_tokens: accounting.estimated_billable_input_tokens,
            saved_input_tokens: accounting.saved_input_tokens,
            input_cache_hit_rate: accounting.input_cache_hit_rate,
            prefix_cache_state: input_cache
                .as_ref()
                .map(|report| report.prefix_cache_state.clone()),
            tool_result_cache_state: input_cache
                .as_ref()
                .map(|report| report.tool_result_cache_state.clone()),
            input_cache_ttl_secs: input_cache
                .as_ref()
                .and_then(|report| report.input_cache_ttl_secs),
            duration_ms: self.started_at.elapsed().as_millis() as u64,
            request_bytes: self.request_bytes,
            response_bytes,
            request: self.request.clone(),
            response,
            error,
        };

        if let Err(e) = store.append(record) {
            tracing::warn!(error = %e, "调用记录写入失败");
        }
    }
}

fn usage_from_json(value: &serde_json::Value) -> UsageParts {
    let usage = value.get("usage").and_then(|v| v.as_object());
    UsageParts {
        input_tokens: usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_i64()),
        output_tokens: usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_i64()),
        cache_read_input_tokens: usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|v| v.as_i64()),
        cache_creation_input_tokens: usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|v| v.as_i64()),
    }
}

fn usage_from_sse_events(events: &[SseEvent]) -> UsageParts {
    let mut usage = UsageParts::default();
    for event in events {
        let source = if event.event == "message_start" {
            event.data.get("message").and_then(|v| v.get("usage"))
        } else if event.event == "message_delta" {
            event.data.get("usage")
        } else {
            None
        };

        if let Some(source) = source {
            usage.input_tokens = source
                .get("input_tokens")
                .and_then(|v| v.as_i64())
                .or(usage.input_tokens);
            usage.output_tokens = source
                .get("output_tokens")
                .and_then(|v| v.as_i64())
                .or(usage.output_tokens);
            usage.cache_read_input_tokens = source
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_i64())
                .or(usage.cache_read_input_tokens);
            usage.cache_creation_input_tokens = source
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_i64())
                .or(usage.cache_creation_input_tokens);
        }
    }
    usage
}

fn response_credential_id(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("x-kiro-credential-id")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// GET /v1/models
///
/// 返回可用的模型列表
pub async fn get_models() -> impl IntoResponse {
    tracing::info!("Received GET /v1/models request");

    let models = vec![
        Model {
            id: "claude-opus-4-7".to_string(),
            object: "model".to_string(),
            created: 1772409600, // Mar 2, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-7-thinking".to_string(),
            object: "model".to_string(),
            created: 1772409600, // Mar 2, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-6".to_string(),
            object: "model".to_string(),
            created: 1770163200, // Feb 4, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1770163200, // Feb 4, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-6".to_string(),
            object: "model".to_string(),
            created: 1771286400, // Feb 17, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1771286400, // Feb 17, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-5-20251101".to_string(),
            object: "model".to_string(),
            created: 1763942400, // Nov 24, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-5-20251101-thinking".to_string(),
            object: "model".to_string(),
            created: 1763942400, // Nov 24, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-5-20250929".to_string(),
            object: "model".to_string(),
            created: 1759104000, // Sep 29, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-5-20250929-thinking".to_string(),
            object: "model".to_string(),
            created: 1759104000, // Sep 29, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-haiku-4-5-20251001".to_string(),
            object: "model".to_string(),
            created: 1760486400, // Oct 15, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-haiku-4-5-20251001-thinking".to_string(),
            object: "model".to_string(),
            created: 1760486400, // Oct 15, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
    ];

    Json(ModelsResponse {
        object: "list".to_string(),
        data: models,
    })
}

/// POST /v1/messages
///
/// 创建消息（对话）
pub async fn post_messages(
    State(state): State<AppState>,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
) -> Response {
    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        "Received POST /v1/messages request"
    );
    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            tracing::error!("KiroProvider 未配置");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Kiro API provider not configured",
                )),
            )
                .into_response();
        }
    };

    // 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
    override_thinking_from_model_name(&mut payload);
    let request_log = serde_json::to_value(&payload).ok();

    // 检查是否为 WebSearch 请求
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到 WebSearch 工具，路由到 WebSearch 处理");

        // 估算输入 tokens
        let input_tokens = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;

        return websearch::handle_websearch_request(provider, &payload, input_tokens).await;
    }

    // 转换请求
    let conversion_result = match convert_request(&payload) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 构建 Kiro 请求（profile_arn 由 provider 层根据实际凭据注入）
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: None,
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    tracing::debug!("Kiro request body: {}", request_body);

    let response_cache_key = if state.true_cache.is_some() && is_true_cache_eligible(&payload) {
        response_cache_key(&payload, &request_body)
    } else {
        None
    };

    // 估算输入 tokens
    let input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;
    let input_cache_report = state
        .input_cache
        .as_ref()
        .map(|cache| cache.analyze_and_store(&payload, input_tokens as i64));
    let call_log = CallLogContext::new(
        state.call_log_store.clone(),
        "/v1/messages",
        &payload.model,
        payload.stream,
        request_log,
        response_cache_key.clone(),
        input_cache_report,
    );

    // 检查是否启用了thinking
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .map(|t| t.is_enabled())
        .unwrap_or(false);

    let tool_name_map = conversion_result.tool_name_map;

    if payload.stream {
        // 流式响应
        handle_stream_request(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            thinking_enabled,
            tool_name_map,
            state.true_cache.clone(),
            response_cache_key,
            call_log,
        )
        .await
    } else {
        // 非流式响应：仅在配置开启时提取 thinking 块
        let extract_thinking = state.extract_thinking && thinking_enabled;
        handle_non_stream_request(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            extract_thinking,
            tool_name_map,
            state.true_cache.clone(),
            response_cache_key,
            call_log,
        )
        .await
    }
}

/// 处理流式请求
async fn handle_stream_request(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    true_cache: Option<std::sync::Arc<TrueCache>>,
    response_cache_key: Option<String>,
    call_log: CallLogContext,
) -> Response {
    if let (Some(cache), Some(key)) = (&true_cache, &response_cache_key) {
        if let Some(cached) = cache.get_response(key) {
            if let Some(events) = sse_events_from_cache_value(&cached) {
                tracing::info!(cache_key = %key, "流式真缓存命中，跳过上游 Kiro API");
                let marked_events = mark_cache_hit_sse_events(events);
                call_log.success(
                    200,
                    "hit",
                    None,
                    Some(sse_events_to_cache_value(&marked_events)),
                    usage_from_sse_events(&marked_events),
                );
                return ok_sse_with_cache_header(marked_events, "hit");
            }
            tracing::warn!(cache_key = %key, "流式真缓存文件格式不匹配，回退上游请求");
        }
    }

    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider.call_api_stream(request_body).await {
        Ok(resp) => resp,
        Err(e) => {
            let error = e.to_string();
            call_log.error(502, "error", None, error, None);
            return map_provider_error(e);
        }
    };
    let credential_id = response_credential_id(&response);

    // 创建流处理上下文
    let mut ctx =
        StreamContext::new_with_thinking(model, input_tokens, thinking_enabled, tool_name_map);

    // 生成初始事件
    let mut initial_events = ctx.generate_initial_events();
    zero_stream_message_start_usage(&mut initial_events);

    // 创建 SSE 流。可缓存的流式 miss 边转发边记录，流结束后落盘供后续直接回放。
    let (stream, cache_state): (
        std::pin::Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send>>,
        &'static str,
    ) = if let (Some(cache), Some(key)) = (true_cache, response_cache_key) {
        (
            Box::pin(create_cacheable_sse_stream(
                response,
                ctx,
                initial_events,
                cache,
                key,
                call_log,
                credential_id,
            )),
            "miss-streaming",
        )
    } else {
        (
            Box::pin(create_sse_stream(
                response,
                ctx,
                initial_events,
                call_log,
                credential_id,
            )),
            "bypass",
        )
    };

    // 返回 SSE 响应
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header("x-kiro-true-cache", cache_state)
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Ping 事件间隔（25秒）
const PING_INTERVAL_SECS: u64 = 25;

/// 创建 ping 事件的 SSE 字符串
fn create_ping_sse() -> Bytes {
    Bytes::from("event: ping\ndata: {\"type\": \"ping\"}\n\n")
}

/// 创建 SSE 事件流
fn create_sse_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    initial_events: Vec<SseEvent>,
    call_log: CallLogContext,
    credential_id: Option<u64>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let logged_initial_events = initial_events.clone();
    // 先发送初始事件
    let initial_stream = stream::iter(
        initial_events
            .into_iter()
            .map(|e| Ok(Bytes::from(e.to_sse_string()))),
    );

    // 然后处理 Kiro 响应流，同时每25秒发送 ping 保活
    let body_stream = response.bytes_stream();

    let processing_stream = stream::unfold(
        (
            body_stream,
            ctx,
            EventStreamDecoder::new(),
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
            logged_initial_events,
            call_log,
            credential_id,
        ),
        |(
            mut body_stream,
            mut ctx,
            mut decoder,
            finished,
            mut ping_interval,
            mut logged_events,
            call_log,
            credential_id,
        )| async move {
            if finished {
                return None;
            }

            // 使用 select! 同时等待数据和 ping 定时器
            tokio::select! {
                // 处理数据流
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            // 解码事件
                            if let Err(e) = decoder.feed(&chunk) {
                                tracing::warn!("缓冲区溢出: {}", e);
                            }

                            let mut events = Vec::new();
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => {
                                        if let Ok(event) = Event::from_frame(frame) {
                                            let sse_events = ctx.process_kiro_event(&event);
                                            events.extend(sse_events);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("解码事件失败: {}", e);
                                    }
                                }
                            }

                            // 转换为 SSE 字节流
                            logged_events.extend(events.iter().cloned());
                            let bytes: Vec<Result<Bytes, Infallible>> = events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();

                            Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval, logged_events, call_log, credential_id)))
                        }
                        Some(Err(e)) => {
                            tracing::error!("读取响应流失败: {}", e);
                            // 发送最终事件并结束
                            let final_events = ctx.generate_final_events();
                            logged_events.extend(final_events.iter().cloned());
                            call_log.error(
                                502,
                                "bypass",
                                credential_id,
                                format!("读取响应流失败: {}", e),
                                Some(sse_events_to_cache_value(&logged_events)),
                            );
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, logged_events, call_log, credential_id)))
                        }
                        None => {
                            // 流结束，发送最终事件
                            let mut final_events = ctx.generate_final_events();
                            apply_input_cache_usage_to_sse_events(
                                &mut final_events,
                                call_log.input_cache_report.as_ref(),
                            );
                            let empty_output_zeroed =
                                zero_empty_visible_sse_usage(&logged_events, &mut final_events);
                            logged_events.extend(final_events.iter().cloned());
                            let log_cache_state = if empty_output_zeroed {
                                "empty-output-zeroed"
                            } else {
                                "bypass"
                            };
                            call_log.success(
                                200,
                                log_cache_state,
                                credential_id,
                                Some(sse_events_to_cache_value(&logged_events)),
                                usage_from_sse_events(&logged_events),
                            );
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, logged_events, call_log, credential_id)))
                        }
                    }
                }
                // 发送 ping 保活
                _ = ping_interval.tick() => {
                    tracing::trace!("发送 ping 保活事件");
                    let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                    Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval, logged_events, call_log, credential_id)))
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

/// 创建可缓存的 SSE 事件流。
///
/// 首次请求会缓冲完整上游流，并在结束后一次性回放给客户端，同时把 SSE 事件落盘。
/// 后续相同请求可直接从磁盘回放，从而真正跳过上游 Kiro API。
fn create_cacheable_sse_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    initial_events: Vec<SseEvent>,
    cache: std::sync::Arc<TrueCache>,
    cache_key: String,
    call_log: CallLogContext,
    credential_id: Option<u64>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let body_stream = response.bytes_stream();
    let buffered_initial_events = initial_events.clone();
    let initial_stream = stream::iter(
        initial_events
            .into_iter()
            .map(|event| Ok::<Bytes, Infallible>(Bytes::from(event.to_sse_string()))),
    );

    let processing_stream = stream::unfold(
        (
            body_stream,
            ctx,
            EventStreamDecoder::new(),
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
            buffered_initial_events,
            cache,
            cache_key,
            call_log,
            credential_id,
        ),
        |(
            mut body_stream,
            mut ctx,
            mut decoder,
            finished,
            mut ping_interval,
            mut buffered_events,
            cache,
            cache_key,
            call_log,
            credential_id,
        )| async move {
            if finished {
                return None;
            }

            loop {
                tokio::select! {
                    biased;

                    _ = ping_interval.tick() => {
                        tracing::trace!("发送 ping 保活事件（真缓存记录模式）");
                        let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                        return Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval, buffered_events, cache, cache_key, call_log, credential_id)));
                    }

                    chunk_result = body_stream.next() => {
                        match chunk_result {
                            Some(Ok(chunk)) => {
                                if let Err(e) = decoder.feed(&chunk) {
                                    tracing::warn!("缓冲区溢出: {}", e);
                                }

                                let mut output_events = Vec::new();
                                for result in decoder.decode_iter() {
                                    match result {
                                        Ok(frame) => {
                                            if let Ok(event) = Event::from_frame(frame) {
                                                let sse_events = ctx.process_kiro_event(&event);
                                                buffered_events.extend(sse_events.iter().cloned());
                                                output_events.extend(sse_events);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!("解码事件失败: {}", e);
                                        }
                                    }
                                }

                                if !output_events.is_empty() {
                                    let bytes: Vec<Result<Bytes, Infallible>> = output_events
                                        .into_iter()
                                        .map(|event| Ok(Bytes::from(event.to_sse_string())))
                                        .collect();
                                    return Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval, buffered_events, cache, cache_key, call_log, credential_id)));
                                }
                            }
                            Some(Err(e)) => {
                                tracing::error!("读取响应流失败: {}", e);
                                let final_events = ctx.generate_final_events();
                                buffered_events.extend(final_events.iter().cloned());
                                call_log.error(
                                    502,
                                    "miss-streaming",
                                    credential_id,
                                    format!("读取响应流失败: {}", e),
                                    Some(sse_events_to_cache_value(&buffered_events)),
                                );
                                let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                    .into_iter()
                                    .map(|event| Ok(Bytes::from(event.to_sse_string())))
                                    .collect();
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, Vec::new(), cache, cache_key, call_log, credential_id)));
                            }
                            None => {
                                let mut final_events = ctx.generate_final_events();
                                apply_input_cache_usage_to_sse_events(
                                    &mut final_events,
                                    call_log.input_cache_report.as_ref(),
                                );
                                let empty_output_zeroed = zero_empty_visible_sse_usage(
                                    &buffered_events,
                                    &mut final_events,
                                );
                                buffered_events.extend(final_events.iter().cloned());
                                let cache_value = sse_events_to_cache_value(&buffered_events);
                                let cache_state = if empty_output_zeroed {
                                    tracing::info!(cache_key = %cache_key, "流式响应没有可见文本，清零 usage 并跳过真缓存写入");
                                    "empty-output-zeroed"
                                } else if sse_events_are_cacheable(&buffered_events) {
                                    match cache.put_response(&cache_key, &cache_value) {
                                        Ok(()) => {
                                            tracing::info!(cache_key = %cache_key, "流式真缓存写入成功");
                                            "miss-stored"
                                        }
                                        Err(e) => {
                                            tracing::warn!(cache_key = %cache_key, error = %e, "流式真缓存写入失败");
                                            "miss-write-failed"
                                        }
                                    }
                                } else {
                                    tracing::debug!(cache_key = %cache_key, "流式响应非 end_turn/tool_use，跳过真缓存写入");
                                    "bypass-tool-output"
                                };
                                call_log.success(
                                    200,
                                    cache_state,
                                    credential_id,
                                    Some(cache_value),
                                    usage_from_sse_events(&buffered_events),
                                );
                                let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                    .into_iter()
                                    .map(|event| Ok(Bytes::from(event.to_sse_string())))
                                    .collect();
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, Vec::new(), cache, cache_key, call_log, credential_id)));
                            }
                        }
                    }
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::{Message, Tool};
    use std::collections::HashMap;

    fn request_with_tool() -> MessagesRequest {
        MessagesRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 1024,
            messages: vec![Message {
                role: "user".to_string(),
                content: json!("hello"),
            }],
            stream: true,
            system: None,
            tools: Some(vec![Tool {
                tool_type: None,
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: HashMap::new(),
                max_uses: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn true_cache_eligibility_allows_passive_tools() {
        let mut request = request_with_tool();
        assert!(is_true_cache_eligible(&request));

        request.tool_choice = Some(json!({ "type": "auto" }));
        assert!(is_true_cache_eligible(&request));

        request.tool_choice = Some(json!({ "type": "none" }));
        assert!(is_true_cache_eligible(&request));
    }

    #[test]
    fn true_cache_eligibility_rejects_forced_tool_choices() {
        let mut request = request_with_tool();

        request.tool_choice = Some(json!({ "type": "any" }));
        assert!(!is_true_cache_eligible(&request));

        request.tool_choice = Some(json!({ "type": "tool", "name": "read_file" }));
        assert!(!is_true_cache_eligible(&request));
    }

    #[test]
    fn sse_cacheability_accepts_complete_text_or_tool_use() {
        let text_events = vec![
            SseEvent::new(
                "content_block_start",
                json!({ "content_block": { "type": "text" } }),
            ),
            SseEvent::new(
                "message_delta",
                json!({ "delta": { "stop_reason": "end_turn" } }),
            ),
        ];
        assert!(sse_events_are_cacheable(&text_events));

        let tool_events = vec![
            SseEvent::new(
                "content_block_start",
                json!({ "content_block": { "type": "tool_use" } }),
            ),
            SseEvent::new(
                "message_delta",
                json!({ "delta": { "stop_reason": "tool_use" } }),
            ),
        ];
        assert!(sse_events_are_cacheable(&tool_events));

        let max_tokens_events = vec![SseEvent::new(
            "message_delta",
            json!({ "delta": { "stop_reason": "max_tokens" } }),
        )];
        assert!(!sse_events_are_cacheable(&max_tokens_events));
    }

    #[test]
    fn empty_visible_sse_output_zeroes_final_usage() {
        let history = Vec::new();
        let mut final_events = vec![SseEvent::new(
            "message_delta",
            json!({
                "delta": { "stop_reason": "end_turn" },
                "usage": {
                    "input_tokens": 100,
                    "cache_read_input_tokens": 900,
                    "cache_creation_input_tokens": 0,
                    "output_tokens": 10
                }
            }),
        )];

        assert!(zero_empty_visible_sse_usage(&history, &mut final_events));
        assert_eq!(final_events[0].data["usage"]["input_tokens"], 0);
        assert_eq!(final_events[0].data["usage"]["cache_read_input_tokens"], 0);
        assert_eq!(
            final_events[0].data["usage"]["cache_creation_input_tokens"],
            0
        );
        assert_eq!(final_events[0].data["usage"]["output_tokens"], 0);
    }

    #[test]
    fn sse_tool_use_keeps_usage() {
        let history = vec![SseEvent::new(
            "content_block_start",
            json!({ "content_block": { "type": "tool_use" } }),
        )];
        let mut final_events = vec![SseEvent::new(
            "message_delta",
            json!({
                "delta": { "stop_reason": "tool_use" },
                "usage": {
                    "input_tokens": 100,
                    "cache_read_input_tokens": 900,
                    "cache_creation_input_tokens": 0,
                    "output_tokens": 10
                }
            }),
        )];

        assert!(!zero_empty_visible_sse_usage(&history, &mut final_events));
        assert_eq!(final_events[0].data["usage"]["input_tokens"], 100);
        assert_eq!(
            final_events[0].data["usage"]["cache_read_input_tokens"],
            900
        );
        assert_eq!(final_events[0].data["usage"]["output_tokens"], 10);
    }

    #[test]
    fn visible_sse_text_keeps_usage() {
        let history = vec![SseEvent::new(
            "content_block_delta",
            json!({ "delta": { "type": "text_delta", "text": "hello" } }),
        )];
        let mut final_events = vec![SseEvent::new(
            "message_delta",
            json!({
                "delta": { "stop_reason": "end_turn" },
                "usage": { "input_tokens": 100, "output_tokens": 2 }
            }),
        )];

        assert!(!zero_empty_visible_sse_usage(&history, &mut final_events));
        assert_eq!(final_events[0].data["usage"]["input_tokens"], 100);
        assert_eq!(final_events[0].data["usage"]["output_tokens"], 2);
    }

    #[test]
    fn stream_message_start_usage_is_zeroed_before_final_delta() {
        let mut events = vec![SseEvent::new(
            "message_start",
            json!({
                "message": {
                    "usage": {
                        "input_tokens": 100,
                        "cache_read_input_tokens": 900,
                        "cache_creation_input_tokens": 0,
                        "output_tokens": 1
                    }
                }
            }),
        )];

        assert!(zero_stream_message_start_usage(&mut events));
        assert_eq!(events[0].data["message"]["usage"]["input_tokens"], 0);
        assert_eq!(
            events[0].data["message"]["usage"]["cache_read_input_tokens"],
            0
        );
        assert_eq!(
            events[0].data["message"]["usage"]["cache_creation_input_tokens"],
            0
        );
        assert_eq!(events[0].data["message"]["usage"]["output_tokens"], 0);
    }

    #[test]
    fn empty_visible_json_output_zeroes_usage() {
        let mut response = json!({
            "content": [],
            "usage": {
                "input_tokens": 100,
                "cache_read_input_tokens": 900,
                "cache_creation_input_tokens": 0,
                "output_tokens": 10
            }
        });

        assert!(zero_empty_visible_json_usage(&mut response));
        assert_eq!(response["usage"]["input_tokens"], 0);
        assert_eq!(response["usage"]["cache_read_input_tokens"], 0);
        assert_eq!(response["usage"]["cache_creation_input_tokens"], 0);
        assert_eq!(response["usage"]["output_tokens"], 0);
    }

    #[test]
    fn json_tool_use_keeps_usage() {
        let mut response = json!({
            "content": [
                { "type": "tool_use", "id": "tool_1", "name": "read_file", "input": {} }
            ],
            "usage": {
                "input_tokens": 100,
                "cache_read_input_tokens": 900,
                "cache_creation_input_tokens": 0,
                "output_tokens": 10
            }
        });

        assert!(!zero_empty_visible_json_usage(&mut response));
        assert_eq!(response["usage"]["input_tokens"], 100);
        assert_eq!(response["usage"]["cache_read_input_tokens"], 900);
        assert_eq!(response["usage"]["output_tokens"], 10);
    }

    #[test]
    fn visible_json_text_keeps_usage() {
        let mut response = json!({
            "content": [{ "type": "text", "text": "hello" }],
            "usage": { "input_tokens": 100, "output_tokens": 2 }
        });

        assert!(!zero_empty_visible_json_usage(&mut response));
        assert_eq!(response["usage"]["input_tokens"], 100);
        assert_eq!(response["usage"]["output_tokens"], 2);
    }

    #[test]
    fn cache_hit_json_refreshes_tool_use_ids() {
        let cached = json!({
            "id": "msg_cached",
            "type": "message",
            "content": [{
                "type": "tool_use",
                "id": "tooluse_cached",
                "name": "read_file",
                "input": {"path": "/tmp/a.txt"}
            }],
            "usage": {"input_tokens": 10, "output_tokens": 2}
        });

        let marked = mark_cache_hit_usage(cached);
        assert_ne!(marked["id"], "msg_cached");
        assert_ne!(marked["content"][0]["id"], "tooluse_cached");
        assert_eq!(marked["content"][0]["name"], "read_file");
        assert_eq!(marked["usage"]["input_tokens"], 0);
        assert_eq!(marked["usage"]["cache_read_input_tokens"], 10);
    }

    #[test]
    fn cache_hit_json_preserves_adjusted_input_total() {
        let cached = json!({
            "id": "msg_cached",
            "type": "message",
            "content": [],
            "usage": {
                "input_tokens": 30,
                "cache_read_input_tokens": 120,
                "cache_creation_input_tokens": 0,
                "output_tokens": 2
            }
        });

        let marked = mark_cache_hit_usage(cached);

        assert_eq!(marked["usage"]["input_tokens"], 0);
        assert_eq!(marked["usage"]["cache_read_input_tokens"], 150);
        assert_eq!(marked["usage"]["cache_creation_input_tokens"], 0);
    }

    #[test]
    fn cache_hit_sse_refreshes_tool_use_ids() {
        let events = vec![
            SseEvent::new(
                "message_start",
                json!({ "message": { "id": "msg_cached", "usage": { "input_tokens": 10 } } }),
            ),
            SseEvent::new(
                "content_block_start",
                json!({ "content_block": { "type": "tool_use", "id": "tooluse_cached", "name": "read_file" } }),
            ),
            SseEvent::new(
                "message_delta",
                json!({ "delta": { "stop_reason": "tool_use" }, "usage": { "input_tokens": 10, "output_tokens": 2 } }),
            ),
        ];

        let marked = mark_cache_hit_sse_events(events);
        assert_ne!(marked[0].data["message"]["id"], "msg_cached");
        assert_ne!(marked[1].data["content_block"]["id"], "tooluse_cached");
        assert_eq!(marked[1].data["content_block"]["name"], "read_file");
        assert_eq!(marked[2].data["usage"]["input_tokens"], 0);
        assert_eq!(marked[2].data["usage"]["cache_read_input_tokens"], 10);
    }

    #[test]
    fn cache_hit_sse_preserves_adjusted_input_total() {
        let events = vec![
            SseEvent::new(
                "message_start",
                json!({
                    "message": {
                        "id": "msg_cached",
                        "usage": {
                            "input_tokens": 30,
                            "cache_read_input_tokens": 120,
                            "cache_creation_input_tokens": 0
                        }
                    }
                }),
            ),
            SseEvent::new(
                "message_delta",
                json!({
                    "delta": { "stop_reason": "end_turn" },
                    "usage": {
                        "input_tokens": 30,
                        "cache_read_input_tokens": 120,
                        "cache_creation_input_tokens": 0,
                        "output_tokens": 2
                    }
                }),
            ),
        ];

        let marked = mark_cache_hit_sse_events(events);

        assert_eq!(marked[1].data["usage"]["input_tokens"], 0);
        assert_eq!(marked[1].data["usage"]["cache_read_input_tokens"], 150);
        assert_eq!(marked[1].data["usage"]["cache_creation_input_tokens"], 0);
    }

    #[test]
    fn input_cache_usage_is_written_to_json_usage() {
        let report = InputCacheReport {
            raw_input_tokens: 100,
            estimated_billable_input_tokens: 20,
            saved_input_tokens: 80,
            input_cache_hit_rate: 0.8,
            prefix_cache_state: "partial-hit".to_string(),
            tool_result_cache_state: "bypass".to_string(),
            input_cache_ttl_secs: Some(300),
        };
        let mut response = json!({
            "id": "msg_test",
            "type": "message",
            "content": [],
            "usage": {
                "input_tokens": 150,
                "output_tokens": 2
            }
        });

        assert!(apply_input_cache_usage_to_json(
            &mut response,
            Some(&report)
        ));

        assert_eq!(response["usage"]["input_tokens"], 30);
        assert_eq!(response["usage"]["cache_read_input_tokens"], 120);
        assert_eq!(response["usage"]["cache_creation_input_tokens"], 0);
        assert_eq!(response["usage"]["output_tokens"], 2);
    }

    #[test]
    fn input_cache_usage_is_written_to_sse_usage() {
        let report = InputCacheReport {
            raw_input_tokens: 100,
            estimated_billable_input_tokens: 20,
            saved_input_tokens: 80,
            input_cache_hit_rate: 0.8,
            prefix_cache_state: "partial-hit".to_string(),
            tool_result_cache_state: "bypass".to_string(),
            input_cache_ttl_secs: Some(300),
        };
        let mut events = vec![
            SseEvent::new(
                "message_start",
                json!({ "message": { "id": "msg_test", "usage": { "input_tokens": 150 } } }),
            ),
            SseEvent::new(
                "message_delta",
                json!({ "delta": { "stop_reason": "end_turn" }, "usage": { "input_tokens": 150, "output_tokens": 2 } }),
            ),
        ];

        assert!(apply_input_cache_usage_to_sse_events(
            &mut events,
            Some(&report)
        ));

        assert_eq!(events[0].data["message"]["usage"]["input_tokens"], 30);
        assert_eq!(
            events[0].data["message"]["usage"]["cache_read_input_tokens"],
            120
        );
        assert_eq!(events[1].data["usage"]["input_tokens"], 30);
        assert_eq!(events[1].data["usage"]["cache_read_input_tokens"], 120);
        assert_eq!(events[1].data["usage"]["output_tokens"], 2);
    }

    #[test]
    fn accounting_uses_upstream_cache_tokens_as_raw_floor() {
        let report = InputCacheReport {
            raw_input_tokens: 145,
            estimated_billable_input_tokens: 0,
            saved_input_tokens: 145,
            input_cache_hit_rate: 1.0,
            prefix_cache_state: "hit".to_string(),
            tool_result_cache_state: "bypass".to_string(),
            input_cache_ttl_secs: Some(3600),
        };
        let usage = UsageParts {
            input_tokens: Some(0),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(4273),
            cache_creation_input_tokens: Some(0),
        };

        let accounting = compute_input_cache_accounting(Some(&report), &usage);

        assert_eq!(accounting.raw_input_tokens, Some(4273));
        assert_eq!(accounting.saved_input_tokens, Some(4273));
        assert_eq!(accounting.estimated_billable_input_tokens, Some(0));
        assert_eq!(accounting.input_cache_hit_rate, Some(1.0));
    }

    #[test]
    fn accounting_caps_saved_tokens_to_raw_tokens() {
        let usage = UsageParts {
            input_tokens: Some(20),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(80),
            cache_creation_input_tokens: Some(0),
        };

        let accounting = compute_input_cache_accounting(None, &usage);

        assert_eq!(accounting.raw_input_tokens, Some(100));
        assert_eq!(accounting.saved_input_tokens, Some(80));
        assert_eq!(accounting.estimated_billable_input_tokens, Some(20));
        assert_eq!(accounting.input_cache_hit_rate, Some(0.8));
    }

    #[test]
    fn accounting_scales_local_cache_hit_rate_to_upstream_raw_tokens() {
        let report = InputCacheReport {
            raw_input_tokens: 100,
            estimated_billable_input_tokens: 20,
            saved_input_tokens: 80,
            input_cache_hit_rate: 0.8,
            prefix_cache_state: "partial-hit".to_string(),
            tool_result_cache_state: "bypass".to_string(),
            input_cache_ttl_secs: Some(300),
        };
        let usage = UsageParts {
            input_tokens: Some(150),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        };

        let accounting = compute_input_cache_accounting(Some(&report), &usage);

        assert_eq!(accounting.raw_input_tokens, Some(150));
        assert_eq!(accounting.saved_input_tokens, Some(120));
        assert_eq!(accounting.estimated_billable_input_tokens, Some(30));
        assert_eq!(accounting.input_cache_hit_rate, Some(0.8));
    }
}

use super::converter::get_context_window_size;

/// 处理非流式请求
async fn handle_non_stream_request(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    true_cache: Option<std::sync::Arc<TrueCache>>,
    response_cache_key: Option<String>,
    call_log: CallLogContext,
) -> Response {
    if let (Some(cache), Some(key)) = (&true_cache, &response_cache_key) {
        if let Some(cached) = cache.get_response(key) {
            tracing::info!(cache_key = %key, "真缓存命中，跳过上游 Kiro API");
            let response_body = mark_cache_hit_usage(cached);
            call_log.success(
                200,
                "hit",
                None,
                Some(response_body.clone()),
                usage_from_json(&response_body),
            );
            return ok_json_with_cache_header(response_body, "hit");
        }
    }

    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider.call_api(request_body).await {
        Ok(resp) => resp,
        Err(e) => {
            let error = e.to_string();
            call_log.error(502, "error", None, error, None);
            return map_provider_error(e);
        }
    };
    let credential_id = response_credential_id(&response);

    // 读取响应体
    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("读取响应体失败: {}", e);
            call_log.error(
                502,
                "error",
                credential_id,
                format!("读取响应失败: {}", e),
                None,
            );
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    format!("读取响应失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    // 解析事件流
    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&body_bytes) {
        tracing::warn!("缓冲区溢出: {}", e);
    }

    let mut text_content = String::new();
    let mut tool_uses: Vec<serde_json::Value> = Vec::new();
    let mut has_tool_use = false;
    let mut stop_reason = "end_turn".to_string();
    // 从 contextUsageEvent 计算的实际输入 tokens
    let mut context_input_tokens: Option<i32> = None;

    // 收集工具调用的增量 JSON
    let mut tool_json_buffers: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => {
                if let Ok(event) = Event::from_frame(frame) {
                    match event {
                        Event::AssistantResponse(resp) => {
                            text_content.push_str(&resp.content);
                        }
                        Event::ToolUse(tool_use) => {
                            has_tool_use = true;

                            // 累积工具的 JSON 输入
                            let buffer = tool_json_buffers
                                .entry(tool_use.tool_use_id.clone())
                                .or_insert_with(String::new);
                            buffer.push_str(&tool_use.input);

                            // 如果是完整的工具调用，添加到列表
                            if tool_use.stop {
                                let input: serde_json::Value = if buffer.is_empty() {
                                    serde_json::json!({})
                                } else {
                                    serde_json::from_str(buffer).unwrap_or_else(|e| {
                                        tracing::warn!(
                                            "工具输入 JSON 解析失败: {}, tool_use_id: {}",
                                            e,
                                            tool_use.tool_use_id
                                        );
                                        serde_json::json!({})
                                    })
                                };

                                let original_name = tool_name_map
                                    .get(&tool_use.name)
                                    .cloned()
                                    .unwrap_or_else(|| tool_use.name.clone());

                                tool_uses.push(json!({
                                    "type": "tool_use",
                                    "id": tool_use.tool_use_id,
                                    "name": original_name,
                                    "input": input
                                }));
                            }
                        }
                        Event::ContextUsage(context_usage) => {
                            // 从上下文使用百分比计算实际的 input_tokens
                            let window_size = get_context_window_size(model);
                            let actual_input_tokens =
                                (context_usage.context_usage_percentage * (window_size as f64)
                                    / 100.0) as i32;
                            context_input_tokens = Some(actual_input_tokens);
                            // 上下文使用量达到 100% 时，设置 stop_reason 为 model_context_window_exceeded
                            if context_usage.context_usage_percentage >= 100.0 {
                                stop_reason = "model_context_window_exceeded".to_string();
                            }
                            tracing::debug!(
                                "收到 contextUsageEvent: {}%, 计算 input_tokens: {}",
                                context_usage.context_usage_percentage,
                                actual_input_tokens
                            );
                        }
                        Event::Exception { exception_type, .. } => {
                            if exception_type == "ContentLengthExceededException" {
                                stop_reason = "max_tokens".to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                tracing::warn!("解码事件失败: {}", e);
            }
        }
    }

    // 确定 stop_reason
    if has_tool_use && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
    }

    // 构建响应内容
    let mut content: Vec<serde_json::Value> = Vec::new();

    if thinking_enabled {
        // 从完整文本中提取 thinking 块
        let (thinking, remaining_text) =
            super::stream::extract_thinking_from_complete_text(&text_content);

        if let Some(thinking_text) = thinking {
            content.push(json!({
                "type": "thinking",
                "thinking": thinking_text
            }));
        }

        if !remaining_text.is_empty() {
            content.push(json!({
                "type": "text",
                "text": remaining_text
            }));
        }
    } else if !text_content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": text_content
        }));
    }

    content.extend(tool_uses);

    // 估算输出 tokens
    let output_tokens = token::estimate_output_tokens(&content);

    // 使用从 contextUsageEvent 计算的 input_tokens，如果没有则使用估算值
    let final_input_tokens = context_input_tokens.unwrap_or(input_tokens);

    // 构建 Anthropic 响应
    let mut response_body = json!({
        "id": format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": final_input_tokens,
            "output_tokens": output_tokens
        }
    });
    apply_input_cache_usage_to_json(&mut response_body, call_log.input_cache_report.as_ref());
    let empty_output_zeroed = zero_empty_visible_json_usage(&mut response_body);

    if let (Some(cache), Some(key)) = (&true_cache, &response_cache_key) {
        if empty_output_zeroed {
            tracing::info!(cache_key = %key, "非流式响应没有可见文本，清零 usage 并跳过真缓存写入");
            call_log.success(
                200,
                "empty-output-zeroed",
                credential_id,
                Some(response_body.clone()),
                usage_from_json(&response_body),
            );
            return ok_json_with_cache_header(response_body, "empty-output-zeroed");
        }

        if stop_reason == "end_turn" || stop_reason == "tool_use" {
            if let Err(e) = cache.put_response(key, &response_body) {
                tracing::warn!(cache_key = %key, error = %e, "写入真缓存失败");
                call_log.success(
                    200,
                    "miss-write-failed",
                    credential_id,
                    Some(response_body.clone()),
                    usage_from_json(&response_body),
                );
                return ok_json_with_cache_header(response_body, "miss-write-failed");
            }
            tracing::info!(cache_key = %key, "真缓存写入成功");
            call_log.success(
                200,
                "miss-stored",
                credential_id,
                Some(response_body.clone()),
                usage_from_json(&response_body),
            );
            return ok_json_with_cache_header(response_body, "miss-stored");
        }

        let cache_state = if has_tool_use || stop_reason == "tool_use" {
            "bypass-tool-output"
        } else {
            "bypass"
        };
        call_log.success(
            200,
            cache_state,
            credential_id,
            Some(response_body.clone()),
            usage_from_json(&response_body),
        );
        return ok_json_with_cache_header(response_body, cache_state);
    }

    call_log.success(
        200,
        if empty_output_zeroed {
            "empty-output-zeroed"
        } else {
            "bypass"
        },
        credential_id,
        Some(response_body.clone()),
        usage_from_json(&response_body),
    );
    (StatusCode::OK, Json(response_body)).into_response()
}

/// 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
///
/// - Opus 4.6：覆写为 adaptive 类型
/// - 其他模型：覆写为 enabled 类型
/// - budget_tokens 固定为 20000
fn override_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model_lower = payload.model.to_lowercase();
    if !model_lower.contains("thinking") {
        return;
    }

    let is_opus_4_6 = model_lower.contains("opus")
        && (model_lower.contains("4-6") || model_lower.contains("4.6"));

    let thinking_type = if is_opus_4_6 { "adaptive" } else { "enabled" };

    tracing::info!(
        model = %payload.model,
        thinking_type = thinking_type,
        "模型名包含 thinking 后缀，覆写 thinking 配置"
    );

    payload.thinking = Some(Thinking {
        thinking_type: thinking_type.to_string(),
        budget_tokens: 20000,
    });

    if is_opus_4_6 {
        payload.output_config = Some(OutputConfig {
            effort: "high".to_string(),
        });
    }
}

/// POST /v1/messages/count_tokens
///
/// 计算消息的 token 数量
pub async fn count_tokens(
    JsonExtractor(payload): JsonExtractor<CountTokensRequest>,
) -> impl IntoResponse {
    tracing::info!(
        model = %payload.model,
        message_count = %payload.messages.len(),
        "Received POST /v1/messages/count_tokens request"
    );

    let total_tokens = token::count_all_tokens(
        payload.model,
        payload.system,
        payload.messages,
        payload.tools,
    ) as i32;

    Json(CountTokensResponse {
        input_tokens: total_tokens.max(1) as i32,
    })
}

/// POST /cc/v1/messages
///
/// Claude Code 兼容端点，与 /v1/messages 的区别在于：
/// - 流式响应会等待 kiro 端返回 contextUsageEvent 后再发送 message_start
/// - message_start 中的 input_tokens 是从 contextUsageEvent 计算的准确值
pub async fn post_messages_cc(
    State(state): State<AppState>,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
) -> Response {
    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        "Received POST /cc/v1/messages request"
    );

    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            tracing::error!("KiroProvider 未配置");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Kiro API provider not configured",
                )),
            )
                .into_response();
        }
    };

    // 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
    override_thinking_from_model_name(&mut payload);
    let request_log = serde_json::to_value(&payload).ok();

    // 检查是否为 WebSearch 请求
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到 WebSearch 工具，路由到 WebSearch 处理");

        // 估算输入 tokens
        let input_tokens = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;

        return websearch::handle_websearch_request(provider, &payload, input_tokens).await;
    }

    // 转换请求
    let conversion_result = match convert_request(&payload) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 构建 Kiro 请求（profile_arn 由 provider 层根据实际凭据注入）
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: None,
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    tracing::debug!("Kiro request body: {}", request_body);

    let response_cache_key = if state.true_cache.is_some() && is_true_cache_eligible(&payload) {
        response_cache_key(&payload, &request_body)
    } else {
        None
    };

    // 估算输入 tokens
    let input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;
    let input_cache_report = state
        .input_cache
        .as_ref()
        .map(|cache| cache.analyze_and_store(&payload, input_tokens as i64));
    let call_log = CallLogContext::new(
        state.call_log_store.clone(),
        "/cc/v1/messages",
        &payload.model,
        payload.stream,
        request_log,
        response_cache_key.clone(),
        input_cache_report,
    );

    // 检查是否启用了thinking
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .map(|t| t.is_enabled())
        .unwrap_or(false);

    let tool_name_map = conversion_result.tool_name_map;

    if payload.stream {
        // 流式响应（缓冲模式）
        handle_stream_request_buffered(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            thinking_enabled,
            tool_name_map,
            state.true_cache.clone(),
            response_cache_key,
            call_log,
        )
        .await
    } else {
        // 非流式响应：仅在配置开启时提取 thinking 块
        let extract_thinking = state.extract_thinking && thinking_enabled;
        handle_non_stream_request(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            extract_thinking,
            tool_name_map,
            state.true_cache.clone(),
            response_cache_key,
            call_log,
        )
        .await
    }
}

/// 处理流式请求（缓冲版本）
///
/// 与 `handle_stream_request` 不同，此函数会缓冲所有事件直到流结束，
/// 然后用从 contextUsageEvent 计算的正确 input_tokens 生成 message_start 事件。
async fn handle_stream_request_buffered(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    estimated_input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    true_cache: Option<std::sync::Arc<TrueCache>>,
    response_cache_key: Option<String>,
    call_log: CallLogContext,
) -> Response {
    if let (Some(cache), Some(key)) = (&true_cache, &response_cache_key) {
        if let Some(cached) = cache.get_response(key) {
            if let Some(events) = sse_events_from_cache_value(&cached) {
                tracing::info!(cache_key = %key, "缓冲流式真缓存命中，跳过上游 Kiro API");
                let marked_events = mark_cache_hit_sse_events(events);
                call_log.success(
                    200,
                    "hit",
                    None,
                    Some(sse_events_to_cache_value(&marked_events)),
                    usage_from_sse_events(&marked_events),
                );
                return ok_sse_with_cache_header(marked_events, "hit");
            }
            tracing::warn!(cache_key = %key, "缓冲流式真缓存文件格式不匹配，回退上游请求");
        }
    }
    let cache_header_state = if response_cache_key.is_some() {
        "miss"
    } else {
        "bypass"
    };

    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider.call_api_stream(request_body).await {
        Ok(resp) => resp,
        Err(e) => {
            let error = e.to_string();
            call_log.error(502, "error", None, error, None);
            return map_provider_error(e);
        }
    };
    let credential_id = response_credential_id(&response);

    // 创建缓冲流处理上下文
    let ctx = BufferedStreamContext::new(
        model,
        estimated_input_tokens,
        thinking_enabled,
        tool_name_map,
    );

    // 创建缓冲 SSE 流
    let stream = create_buffered_sse_stream(
        response,
        ctx,
        true_cache,
        response_cache_key,
        call_log,
        credential_id,
    );

    // 返回 SSE 响应
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header("x-kiro-true-cache", cache_header_state)
        .body(Body::from_stream(stream))
        .unwrap()
}

/// 创建缓冲 SSE 事件流
///
/// 工作流程：
/// 1. 等待上游流完成，期间只发送 ping 保活信号
/// 2. 使用 StreamContext 的事件处理逻辑处理所有 Kiro 事件，结果缓存
/// 3. 流结束后，用正确的 input_tokens 更正 message_start 事件
/// 4. 一次性发送所有事件
fn create_buffered_sse_stream(
    response: reqwest::Response,
    ctx: BufferedStreamContext,
    true_cache: Option<std::sync::Arc<TrueCache>>,
    response_cache_key: Option<String>,
    call_log: CallLogContext,
    credential_id: Option<u64>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let body_stream = response.bytes_stream();

    stream::unfold(
        (
            body_stream,
            ctx,
            EventStreamDecoder::new(),
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
            true_cache,
            response_cache_key,
            call_log,
            credential_id,
        ),
        |(mut body_stream, mut ctx, mut decoder, finished, mut ping_interval, true_cache, response_cache_key, call_log, credential_id)| async move {
            if finished {
                return None;
            }

            loop {
                tokio::select! {
                    // 使用 biased 模式，优先检查 ping 定时器
                    // 避免在上游 chunk 密集时 ping 被"饿死"
                    biased;

                    // 优先检查 ping 保活（等待期间唯一发送的数据）
                    _ = ping_interval.tick() => {
                        tracing::trace!("发送 ping 保活事件（缓冲模式）");
                        let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                        return Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval, true_cache, response_cache_key, call_log, credential_id)));
                    }

                    // 然后处理数据流
                    chunk_result = body_stream.next() => {
                        match chunk_result {
                            Some(Ok(chunk)) => {
                                // 解码事件
                                if let Err(e) = decoder.feed(&chunk) {
                                    tracing::warn!("缓冲区溢出: {}", e);
                                }

                                for result in decoder.decode_iter() {
                                    match result {
                                        Ok(frame) => {
                                            if let Ok(event) = Event::from_frame(frame) {
                                                // 缓冲事件（复用 StreamContext 的处理逻辑）
                                                ctx.process_and_buffer(&event);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!("解码事件失败: {}", e);
                                        }
                                    }
                                }
                                // 继续读取下一个 chunk，不发送任何数据
                            }
                            Some(Err(e)) => {
                                tracing::error!("读取响应流失败: {}", e);
                                // 发生错误，完成处理并返回所有事件
                                let all_events = ctx.finish_and_get_all_events();
                                call_log.error(
                                    502,
                                    "bypass",
                                    credential_id,
                                    format!("读取响应流失败: {}", e),
                                    Some(sse_events_to_cache_value(&all_events)),
                                );
                                let bytes: Vec<Result<Bytes, Infallible>> = all_events
                                    .into_iter()
                                    .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                    .collect();
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, true_cache, response_cache_key, call_log, credential_id)));
                            }
                            None => {
                                // 流结束，完成处理并返回所有事件（已更正 input_tokens）
                                let mut all_events = ctx.finish_and_get_all_events();
                                apply_input_cache_usage_to_sse_events(
                                    &mut all_events,
                                    call_log.input_cache_report.as_ref(),
                                );
                                let empty_output_zeroed =
                                    zero_empty_visible_sse_usage_in_events(&mut all_events);
                                let cache_value = sse_events_to_cache_value(&all_events);
                                let cache_state = if let (Some(cache), Some(key)) = (&true_cache, &response_cache_key) {
                                    if empty_output_zeroed {
                                        tracing::info!(cache_key = %key, "缓冲流式响应没有可见文本，清零 usage 并跳过真缓存写入");
                                        "empty-output-zeroed"
                                    } else if sse_events_are_cacheable(&all_events) {
                                        match cache.put_response(key, &cache_value) {
                                            Ok(()) => {
                                                tracing::info!(cache_key = %key, "缓冲流式真缓存写入成功");
                                                "miss-stored"
                                            }
                                            Err(e) => {
                                                tracing::warn!(cache_key = %key, error = %e, "缓冲流式真缓存写入失败");
                                                "miss-write-failed"
                                            }
                                        }
                                    } else {
                                        tracing::debug!(cache_key = %key, "缓冲流式响应非 end_turn/tool_use，跳过真缓存写入");
                                        "bypass-tool-output"
                                    }
                                } else {
                                    if empty_output_zeroed {
                                        "empty-output-zeroed"
                                    } else {
                                        "bypass"
                                    }
                                };
                                call_log.success(
                                    200,
                                    cache_state,
                                    credential_id,
                                    Some(cache_value),
                                    usage_from_sse_events(&all_events),
                                );
                                let bytes: Vec<Result<Bytes, Infallible>> = all_events
                                    .into_iter()
                                    .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                    .collect();
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, true_cache, response_cache_key, call_log, credential_id)));
                            }
                        }
                    }
                }
            }
        },
    )
    .flatten()
}
