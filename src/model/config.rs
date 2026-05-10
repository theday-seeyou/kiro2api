use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TlsBackend {
    Rustls,
    NativeTls,
}

impl Default for TlsBackend {
    fn default() -> Self {
        Self::Rustls
    }
}

/// KNA 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_region")]
    pub region: String,

    /// Auth Region（用于 Token 刷新），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_region: Option<String>,

    /// API Region（用于 API 请求），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,

    #[serde(default = "default_kiro_version")]
    pub kiro_version: String,

    #[serde(default)]
    pub machine_id: Option<String>,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default = "default_system_version")]
    pub system_version: String,

    #[serde(default = "default_node_version")]
    pub node_version: String,

    #[serde(default = "default_tls_backend")]
    pub tls_backend: TlsBackend,

    /// 外部 count_tokens API 地址（可选）
    #[serde(default)]
    pub count_tokens_api_url: Option<String>,

    /// count_tokens API 密钥（可选）
    #[serde(default)]
    pub count_tokens_api_key: Option<String>,

    /// count_tokens API 认证类型（可选，"x-api-key" 或 "bearer"，默认 "x-api-key"）
    #[serde(default = "default_count_tokens_auth_type")]
    pub count_tokens_auth_type: String,

    /// HTTP 代理地址（可选）
    /// 支持格式: http://host:port, https://host:port, socks5://host:port
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// 代理认证用户名（可选）
    #[serde(default)]
    pub proxy_username: Option<String>,

    /// 代理认证密码（可选）
    #[serde(default)]
    pub proxy_password: Option<String>,

    /// 代理池：可由 Admin API 分配到账号级代理配置
    #[serde(default)]
    pub proxy_pool: Vec<ProxyPoolItem>,

    /// Admin API 密钥（可选，启用 Admin API 功能）
    #[serde(default)]
    pub admin_api_key: Option<String>,

    /// 负载均衡模式（"priority" 或 "balanced"）
    #[serde(default = "default_load_balancing_mode")]
    pub load_balancing_mode: String,

    /// 是否开启非流式响应的 thinking 块提取（默认 true）
    ///
    /// 启用后，非流式响应中的 `<thinking>...</thinking>` 标签会被解析为
    /// 独立的 `{"type": "thinking", ...}` 内容块,与流式响应行为一致。
    #[serde(default = "default_extract_thinking")]
    pub extract_thinking: bool,

    /// 默认端点名称（凭据未显式指定 endpoint 时使用，默认 "ide"）
    #[serde(default = "default_endpoint")]
    pub default_endpoint: String,

    /// 是否启用真缓存（精确响应缓存 + 请求指纹去随机化）
    #[serde(default)]
    pub true_cache_enabled: bool,

    /// 真缓存目录；未配置时使用配置文件同级目录下的 `true-cache`
    #[serde(default)]
    pub true_cache_dir: Option<String>,

    /// 非流式响应缓存 TTL（秒）
    #[serde(default = "default_true_cache_response_ttl_secs")]
    pub true_cache_response_ttl_secs: u64,

    /// 单个响应缓存文件最大字节数
    #[serde(default = "default_true_cache_max_response_bytes")]
    pub true_cache_max_response_bytes: usize,

    /// 是否启用输入技术缓存统计（按官方 5 分钟 / 1 小时 TTL 思路记录可复用输入 token）
    #[serde(default = "default_input_cache_enabled")]
    pub input_cache_enabled: bool,

    /// 输入技术缓存目录；未配置时使用配置文件同级目录下的 `input-cache`
    #[serde(default)]
    pub input_cache_dir: Option<String>,

    /// 短 TTL 输入缓存（秒），用于历史前缀和工具结果，默认 5 分钟
    #[serde(default = "default_input_cache_short_ttl_secs")]
    pub input_cache_short_ttl_secs: u64,

    /// 长 TTL 输入缓存（秒），用于 system/tools 等稳定前缀，默认 1 小时
    #[serde(default = "default_input_cache_long_ttl_secs")]
    pub input_cache_long_ttl_secs: u64,

    /// 是否启用调用记录（Admin UI / API 可查看）
    #[serde(default)]
    pub call_log_enabled: bool,

    /// 调用记录目录；未配置时使用配置文件同级目录下的 `call-logs`
    #[serde(default)]
    pub call_log_dir: Option<String>,

    /// 调用记录最大保留条数
    #[serde(default = "default_call_log_max_records")]
    pub call_log_max_records: usize,

    /// 单条调用记录中 request/response 最大保留字节数
    #[serde(default = "default_call_log_max_body_bytes")]
    pub call_log_max_body_bytes: usize,

    /// 端点特定的配置
    ///
    /// 键为端点名（如 "ide" / "cli"），值为该端点自由定义的参数对象。
    /// 未在此表出现的端点沿用实现内置默认值。
    #[serde(default)]
    pub endpoints: HashMap<String, serde_json::Value>,

    /// 配置文件路径（运行时元数据，不写入 JSON）
    #[serde(skip)]
    config_path: Option<PathBuf>,
}

/// 代理池条目
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolItem {
    /// 代理池条目 ID
    pub id: String,
    /// 代理地址，支持 http/https/socks5
    pub url: String,
    /// 代理认证用户名（可选）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// 代理认证密码（可选）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// 是否禁用该代理
    #[serde(default)]
    pub disabled: bool,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_kiro_version() -> String {
    "0.11.107".to_string()
}

fn default_system_version() -> String {
    const SYSTEM_VERSIONS: &[&str] = &["darwin#24.6.0", "win32#10.0.22631"];
    SYSTEM_VERSIONS[fastrand::usize(..SYSTEM_VERSIONS.len())].to_string()
}

fn default_node_version() -> String {
    "22.22.0".to_string()
}

fn default_count_tokens_auth_type() -> String {
    "x-api-key".to_string()
}

fn default_tls_backend() -> TlsBackend {
    TlsBackend::Rustls
}

fn default_load_balancing_mode() -> String {
    "priority".to_string()
}

fn default_extract_thinking() -> bool {
    true
}

fn default_endpoint() -> String {
    crate::kiro::endpoint::ide::IDE_ENDPOINT_NAME.to_string()
}

fn default_true_cache_response_ttl_secs() -> u64 {
    6 * 60 * 60
}

fn default_true_cache_max_response_bytes() -> usize {
    4 * 1024 * 1024
}

fn default_input_cache_enabled() -> bool {
    true
}

fn default_input_cache_short_ttl_secs() -> u64 {
    5 * 60
}

fn default_input_cache_long_ttl_secs() -> u64 {
    60 * 60
}

fn default_call_log_max_records() -> usize {
    10_000
}

fn default_call_log_max_body_bytes() -> usize {
    256 * 1024
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            region: default_region(),
            auth_region: None,
            api_region: None,
            kiro_version: default_kiro_version(),
            machine_id: None,
            api_key: None,
            system_version: default_system_version(),
            node_version: default_node_version(),
            tls_backend: default_tls_backend(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: default_count_tokens_auth_type(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            proxy_pool: Vec::new(),
            admin_api_key: None,
            load_balancing_mode: default_load_balancing_mode(),
            extract_thinking: default_extract_thinking(),
            default_endpoint: default_endpoint(),
            true_cache_enabled: false,
            true_cache_dir: None,
            true_cache_response_ttl_secs: default_true_cache_response_ttl_secs(),
            true_cache_max_response_bytes: default_true_cache_max_response_bytes(),
            input_cache_enabled: default_input_cache_enabled(),
            input_cache_dir: None,
            input_cache_short_ttl_secs: default_input_cache_short_ttl_secs(),
            input_cache_long_ttl_secs: default_input_cache_long_ttl_secs(),
            call_log_enabled: false,
            call_log_dir: None,
            call_log_max_records: default_call_log_max_records(),
            call_log_max_body_bytes: default_call_log_max_body_bytes(),
            endpoints: HashMap::new(),
            config_path: None,
        }
    }
}

impl Config {
    /// 获取默认配置文件路径
    pub fn default_config_path() -> &'static str {
        "config.json"
    }

    /// 获取有效的 Auth Region（用于 Token 刷新）
    /// 优先使用 auth_region，未配置时回退到 region
    pub fn effective_auth_region(&self) -> &str {
        self.auth_region.as_deref().unwrap_or(&self.region)
    }

    /// 获取有效的 API Region（用于 API 请求）
    /// 优先使用 api_region，未配置时回退到 region
    pub fn effective_api_region(&self) -> &str {
        self.api_region.as_deref().unwrap_or(&self.region)
    }

    /// 从文件加载配置
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            // 配置文件不存在，返回默认配置
            let mut config = Self::default();
            config.config_path = Some(path.to_path_buf());
            return Ok(config);
        }

        let content = fs::read_to_string(path)?;
        let mut config: Config = serde_json::from_str(&content)?;
        config.config_path = Some(path.to_path_buf());
        Ok(config)
    }

    /// 获取配置文件路径（如果有）
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// 将当前配置写回原始配置文件
    pub fn save(&self) -> anyhow::Result<()> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("配置文件路径未知，无法保存配置"))?;

        let content = serde_json::to_string_pretty(self).context("序列化配置失败")?;
        fs::write(path, content)
            .with_context(|| format!("写入配置文件失败: {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parses_proxy_pool() {
        let json = r#"{
            "proxyPool": [
                {
                    "id": "p1",
                    "url": "socks5://proxy-one:1080",
                    "username": "user",
                    "password": "pass"
                },
                {
                    "id": "p2",
                    "url": "http://proxy-two:8080",
                    "disabled": true
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.proxy_pool.len(), 2);
        assert_eq!(config.proxy_pool[0].id, "p1");
        assert_eq!(config.proxy_pool[0].url, "socks5://proxy-one:1080");
        assert_eq!(config.proxy_pool[0].username.as_deref(), Some("user"));
        assert_eq!(config.proxy_pool[0].password.as_deref(), Some("pass"));
        assert!(config.proxy_pool[1].disabled);
    }
}
