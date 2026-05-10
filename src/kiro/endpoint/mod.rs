//! Kiro 端点抽象
//!
//! 不同 Kiro 端点（如 `ide` / `cli`）在 URL、请求头、请求体上存在差异，
//! 但共享凭据池、Token 刷新、重试逻辑和 AWS event-stream 响应解码。
//!
//! [`KiroEndpoint`] 抽象了请求侧的差异点；`KiroProvider` 持有一个 endpoint 注册表，
//! 按凭据的 `endpoint` 字段选择对应实现。

use reqwest::RequestBuilder;

use crate::kiro::model::credentials::KiroCredentials;
use crate::model::config::Config;

pub mod ide;

pub use ide::IdeEndpoint;

/// Kiro 端点
///
/// 同一个 `KiroProvider` 可持有多个 endpoint 实现，按凭据级字段切换。
pub trait KiroEndpoint: Send + Sync {
    /// 端点名称（对应 credentials.endpoint / config.defaultEndpoint 的取值）
    fn name(&self) -> &'static str;

    /// API endpoint URL
    fn api_url(&self, ctx: &RequestContext<'_>) -> String;

    /// MCP endpoint URL
    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String;

    /// 装饰 API 请求的端点特有 header
    ///
    /// Provider 已经设置好 URL、content-type、Connection 和 body；
    /// 实现负责追加 Authorization、host、user-agent 等端点相关头。
    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder;

    /// 装饰 MCP 请求的端点特有 header
    fn decorate_mcp(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder;

    /// 对已序列化的 API 请求体做端点特有加工（如注入 profileArn）
    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> String;

    /// 对已序列化的 MCP 请求体做端点特有加工（默认不变）
    fn transform_mcp_body(&self, body: &str, _ctx: &RequestContext<'_>) -> String {
        body.to_string()
    }

    /// 判断响应体是否表示"月度配额用尽"（禁用凭据并转移）
    fn is_monthly_request_limit(&self, body: &str) -> bool {
        default_is_monthly_request_limit(body)
    }

    /// 判断响应体是否表示"上游 bearer token 失效"（触发强制刷新）
    fn is_bearer_token_invalid(&self, body: &str) -> bool {
        default_is_bearer_token_invalid(body)
    }

    /// 判断响应是否表示"当前账号被限流"（切换凭据但不按月额度永久禁用）
    fn is_rate_limited(&self, status: u16, body: &str) -> bool {
        default_is_rate_limited(status, body)
    }
}

/// 装饰请求时可用的上下文
///
/// 包含单次调用已确定的所有运行时信息。引用形式避免无谓 clone。
pub struct RequestContext<'a> {
    /// 当前凭据
    pub credentials: &'a KiroCredentials,
    /// 有效的 access token（API Key 凭据下即 kiroApiKey）
    pub token: &'a str,
    /// 当前凭据对应的 machineId
    pub machine_id: &'a str,
    /// 全局配置
    pub config: &'a Config,
}

/// 默认的 MONTHLY_REQUEST_COUNT 判断逻辑
///
/// 同时识别顶层 `reason` 字段和嵌套 `error.reason` 字段。
pub fn default_is_monthly_request_limit(body: &str) -> bool {
    if body.contains("MONTHLY_REQUEST_COUNT") {
        return true;
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };

    if value
        .get("reason")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
    {
        return true;
    }

    value
        .pointer("/error/reason")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
}

/// 默认的 bearer token 失效判断逻辑
pub fn default_is_bearer_token_invalid(body: &str) -> bool {
    body.contains("The bearer token included in the request is invalid")
}

/// 默认的账号限流判断逻辑。
///
/// 429 本身可能只是上游整体高负载，因此这里要求正文同时出现明确的限流/节流信号。
pub fn default_is_rate_limited(status: u16, body: &str) -> bool {
    if body.trim().is_empty() {
        return false;
    }

    let body_lower = body.to_lowercase();
    let has_explicit_signal = [
        "throttlingexception",
        "toomanyrequestsexception",
        "requestlimitexceeded",
        "limitexceededexception",
        "rate exceeded",
        "rate limit",
        "rate_limit",
        "ratelimit",
        "too many requests",
        "too_many_requests",
        "请求过于频繁",
        "已被限流",
    ]
    .iter()
    .any(|needle| body_lower.contains(needle));

    if !has_explicit_signal {
        return false;
    }

    matches!(status, 400 | 403 | 408 | 409 | 429 | 500..=599)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_monthly_request_limit_detects_reason() {
        let body = r#"{"message":"You have reached the limit.","reason":"MONTHLY_REQUEST_COUNT"}"#;
        assert!(default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_default_monthly_request_limit_nested_reason() {
        let body = r#"{"error":{"reason":"MONTHLY_REQUEST_COUNT"}}"#;
        assert!(default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_default_monthly_request_limit_false() {
        let body = r#"{"message":"nope","reason":"DAILY_REQUEST_COUNT"}"#;
        assert!(!default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_default_bearer_token_invalid() {
        assert!(default_is_bearer_token_invalid(
            "The bearer token included in the request is invalid"
        ));
        assert!(!default_is_bearer_token_invalid("unrelated error"));
    }

    #[test]
    fn test_default_is_rate_limited_detects_explicit_signals() {
        assert!(default_is_rate_limited(
            429,
            r#"{"__type":"ThrottlingException","message":"Rate exceeded"}"#
        ));
        assert!(default_is_rate_limited(
            429,
            r#"{"error":{"code":"TooManyRequestsException","message":"too many requests"}}"#
        ));
        assert!(default_is_rate_limited(429, "请求过于频繁，已被限流"));
    }

    #[test]
    fn test_default_is_rate_limited_does_not_treat_plain_429_as_account_limit() {
        assert!(!default_is_rate_limited(429, ""));
        assert!(!default_is_rate_limited(
            429,
            r#"{"message":"upstream high traffic, try later"}"#
        ));
        assert!(!default_is_rate_limited(
            400,
            r#"{"message":"maximum context length exceeded"}"#
        ));
    }
}
