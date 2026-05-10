# kiro.rs

基于 Kiro 的 Anthropic Claude API 兼容网关。当前仓库不是原版上游，而是在老项目基础上做过较多运营向改造的分支，重点是多账号池、代理池、调用记录、缓存观测、NewAPI 兼容 usage 字段和 Admin 管理能力。

> 本项目基于老项目 [hank9999/kiro.rs](https://github.com/hank9999/kiro.rs) 二次开发，并参考了 [caidaoli/kiro2api](https://github.com/caidaoli/kiro2api)、[Quorinex/Kiro-Go](https://github.com/Quorinex/Kiro-Go) 等同类实现的思路。感谢原作者和社区项目。

## 免责声明

本项目仅供学习、研究和自托管测试使用。项目与 AWS、Kiro、Anthropic、Claude 等官方无关，不代表任何官方立场。使用者应自行确认账号、网络、服务条款和数据安全风险，使用本项目导致的任何后果由使用者自行承担。

不要把真实的 `config.json`、`credentials.json`、缓存目录、调用日志或任何账号 token 提交到公开仓库。调用记录和真缓存可能包含请求、响应、工具参数等敏感内容。

## 当前分支说明

这个仓库保留了上游 `kiro.rs` 的核心能力：将 Anthropic `/v1/messages` 请求转换为 Kiro 请求，并返回 Anthropic 兼容响应。同时本分支加入了面向实际运营的功能：

- 多账号池：支持单凭据和数组凭据，按优先级或均衡模式调度。
- 代理池：支持全局代理、账号级代理、`direct` 显式直连，以及 Admin 中批量分配代理。
- 限流检测：账号遇到限流后进入临时冷却，冷却结束后自动恢复候选。
- 调用记录：记录模型、输入输出 token、缓存状态、耗时、账号、请求和响应截断体。
- 真缓存：对完全相同且可缓存的请求保存响应，命中时直接回放并跳过上游调用。
- 输入技术缓存：按 5 分钟和 1 小时 TTL 思路统计可复用输入 token，并写入 Anthropic/NewAPI 兼容的缓存 usage 字段。
- 空返保护：无可见输出时会把 usage 清零，避免空响应被计费；有效 `tool_use` 不会被误判为空返。
- Admin UI/API：管理账号、代理池、调用日志、负载均衡模式、余额刷新等。

缓存效果和业务请求形态强相关。真缓存只对完全相同的请求直接命中；长上下文场景的主要节省通常来自输入技术缓存统计和稳定前缀复用，而不是每次都命中完整响应缓存。

## 快速开始

### 1. 构建

构建二进制前需要先构建嵌入式 Admin UI：

```bash
cd admin-ui
npm install
npm run build
cd ..
cargo build --release
```

也可以使用 `pnpm install && pnpm build` 构建前端；仓库中同时保留了 `package-lock.json` 和 `pnpm-lock.yaml`。

### 2. 最小配置

创建 `config.json`：

```json
{
  "host": "127.0.0.1",
  "port": 8990,
  "apiKey": "sk-kiro-rs-your-client-key",
  "tlsBackend": "rustls",
  "region": "us-east-1",
  "adminApiKey": "sk-admin-your-secret-key",
  "defaultEndpoint": "ide"
}
```

创建 `credentials.json`。可以使用单对象格式，也可以使用数组格式。数组格式更适合账号池：

```json
[
  {
    "refreshToken": "your-refresh-token",
    "expiresAt": "2026-12-31T00:00:00Z",
    "authMethod": "social",
    "email": "account-a@example.com",
    "priority": 0,
    "proxyUrl": "http://127.0.0.1:7890"
  },
  {
    "refreshToken": "your-refresh-token-2",
    "expiresAt": "2026-12-31T00:00:00Z",
    "authMethod": "social",
    "email": "account-b@example.com",
    "priority": 1,
    "proxyUrl": "direct"
  }
]
```

IdC 认证示例：

```json
{
  "refreshToken": "your-refresh-token",
  "expiresAt": "2026-12-31T00:00:00Z",
  "authMethod": "idc",
  "clientId": "your-client-id",
  "clientSecret": "your-client-secret"
}
```

Kiro API Key 凭据示例：

```json
{
  "authMethod": "api_key",
  "kiroApiKey": "ksk_xxxxxxxxxxxxx",
  "email": "api-key-account@example.com"
}
```

### 3. 启动

```bash
./target/release/kiro-rs
```

或显式指定配置文件：

```bash
./target/release/kiro-rs -c /path/to/config.json --credentials /path/to/credentials.json
```

### 4. 验证

```bash
curl http://127.0.0.1:8990/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-your-client-key" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "stream": true,
    "messages": [
      {"role": "user", "content": "Hello"}
    ]
  }'
```

## Docker

```bash
docker compose up -d --build
```

需要把真实 `config.json` 和 `credentials.json` 挂载进容器，参考 `docker-compose.yml`。生产环境建议反代到本服务，并只在内网暴露 Admin。

## 配置

### config.json

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `host` | `127.0.0.1` | 服务监听地址 |
| `port` | `8080` | 服务监听端口 |
| `apiKey` | - | 客户端调用 `/v1` 和 `/cc/v1` 使用的 API Key |
| `region` | `us-east-1` | 默认区域 |
| `authRegion` | - | Token 刷新区域；未配置时回退到 `region` |
| `apiRegion` | - | API 请求区域；未配置时回退到 `region` |
| `kiroVersion` | `0.11.107` | 请求中使用的 Kiro 版本标识 |
| `machineId` | - | 自定义机器码；未配置时自动生成或从凭据派生 |
| `systemVersion` | 随机 | 系统版本标识 |
| `nodeVersion` | `22.22.0` | Node.js 版本标识 |
| `tlsBackend` | `rustls` | TLS 后端，可选 `rustls` / `native-tls` |
| `proxyUrl` | - | 全局代理地址，支持 `http` / `https` / `socks5` |
| `proxyUsername` | - | 全局代理用户名 |
| `proxyPassword` | - | 全局代理密码 |
| `proxyPool` | `[]` | 代理池，供 Admin 批量分配到账号级代理 |
| `adminApiKey` | - | 配置后启用 Admin API 和 Web UI |
| `loadBalancingMode` | `priority` | `priority` 按优先级；`balanced` 均衡分配 |
| `extractThinking` | `true` | 非流式响应中提取 `<thinking>` 为独立内容块 |
| `defaultEndpoint` | `ide` | 凭据未指定 endpoint 时使用的 Kiro 端点 |
| `endpoints` | `{}` | 端点特定配置，保留给不同 Kiro 端点扩展 |
| `countTokensApiUrl` | - | 外部 count_tokens API 地址 |
| `countTokensApiKey` | - | 外部 count_tokens API Key |
| `countTokensAuthType` | `x-api-key` | 外部 count_tokens 认证方式：`x-api-key` / `bearer` |
| `trueCacheEnabled` | `false` | 是否启用完整响应真缓存 |
| `trueCacheDir` | `true-cache` | 真缓存目录 |
| `trueCacheResponseTtlSecs` | `21600` | 完整响应缓存 TTL，默认 6 小时 |
| `trueCacheMaxResponseBytes` | `4194304` | 单个响应缓存最大字节数 |
| `inputCacheEnabled` | `true` | 是否启用输入技术缓存统计 |
| `inputCacheDir` | `input-cache` | 输入技术缓存目录 |
| `inputCacheShortTtlSecs` | `300` | 短 TTL，默认 5 分钟 |
| `inputCacheLongTtlSecs` | `3600` | 长 TTL，默认 1 小时 |
| `callLogEnabled` | `false` | 是否启用调用记录 |
| `callLogDir` | `call-logs` | 调用记录目录 |
| `callLogMaxRecords` | `10000` | 最大保留调用记录条数 |
| `callLogMaxBodyBytes` | `262144` | 单条记录中请求/响应体最大保留字节数 |

完整示例可直接参考 `config.example.json`：

```json
{
  "host": "127.0.0.1",
  "port": 8990,
  "apiKey": "sk-kiro-rs-qazWSXedcRFV123456",
  "tlsBackend": "rustls",
  "region": "us-east-1",
  "adminApiKey": "sk-admin-your-secret-key",
  "proxyPool": [],
  "defaultEndpoint": "ide",
  "trueCacheEnabled": false,
  "trueCacheDir": "true-cache",
  "trueCacheResponseTtlSecs": 21600,
  "trueCacheMaxResponseBytes": 4194304,
  "inputCacheEnabled": true,
  "inputCacheDir": "input-cache",
  "inputCacheShortTtlSecs": 300,
  "inputCacheLongTtlSecs": 3600,
  "callLogEnabled": false,
  "callLogDir": "call-logs",
  "callLogMaxRecords": 10000,
  "callLogMaxBodyBytes": 262144
}
```

### credentials.json

凭据支持单对象和数组两种格式。数组格式会按 `priority` 排序，数字越小优先级越高。

| 字段 | 说明 |
| --- | --- |
| `refreshToken` | OAuth 刷新 token |
| `accessToken` | 可选，过期或缺失时会自动刷新 |
| `expiresAt` | token 过期时间，RFC3339 |
| `authMethod` | `social` / `idc` / `api_key` |
| `clientId` / `clientSecret` | IdC 认证需要 |
| `kiroApiKey` | API Key 凭据需要，格式通常为 `ksk_...` |
| `email` | 展示用邮箱 |
| `priority` | 调度优先级 |
| `region` / `authRegion` / `apiRegion` | 凭据级区域配置 |
| `machineId` | 凭据级机器码 |
| `proxyUrl` | 账号级代理；特殊值 `direct` 表示显式直连 |
| `proxyUsername` / `proxyPassword` | 账号级代理认证 |
| `disabled` | 是否禁用该账号 |
| `endpoint` | 该账号使用的 Kiro 端点，未配置时使用 `defaultEndpoint` |

## 缓存机制

### 真缓存

`trueCacheEnabled=true` 后，服务会对请求体生成稳定指纹。相同请求命中时，直接返回缓存响应，并在响应头中写入：

```text
x-kiro-true-cache: hit
```

常见状态：

- `hit`：完整响应真缓存命中，跳过上游调用。
- `miss-stored`：本次未命中，但响应已写入真缓存。
- `miss-streaming`：流式请求正在边转发边收集，结束后可能写入缓存。
- `miss-write-failed`：响应可缓存，但写入缓存失败。
- `bypass`：未启用缓存、不可缓存或没有缓存 key。
- `bypass-tool-output`：工具输出等场景跳过完整响应缓存。
- `empty-output-zeroed`：空可见输出已清零 usage，并跳过真缓存写入。

真缓存适合重复请求、重放测试、固定提示词问答等场景。对每次上下文都变化的聊天请求，完整响应命中率通常不会高。

### 输入技术缓存

`inputCacheEnabled=true` 默认开启。它会按稳定前缀、system/tools、历史消息和工具结果等维度记录可复用输入 token，并把可复用部分写入 Anthropic 兼容 usage 字段，例如：

```json
{
  "usage": {
    "input_tokens": 1200,
    "output_tokens": 80,
    "cache_read_input_tokens": 900,
    "cache_creation_input_tokens": 300
  }
}
```

这部分更适合让 NewAPI 等面板读取缓存效果，也更接近长上下文场景的实际节省来源。它不是官方 Kiro 缓存承诺，也不保证每次都跳过上游请求；实际命中率取决于请求前缀是否稳定、历史消息是否复用、客户端是否每次加入随机内容。

默认 TTL：

- `inputCacheShortTtlSecs=300`：5 分钟，适合会话历史、工具结果等短期复用内容。
- `inputCacheLongTtlSecs=3600`：1 小时，适合 system、tools 等稳定前缀。

## 调用记录

开启：

```json
{
  "callLogEnabled": true,
  "callLogDir": "call-logs",
  "callLogMaxRecords": 10000,
  "callLogMaxBodyBytes": 262144
}
```

记录内容包括模型、状态码、缓存状态、请求时间、耗时、使用账号、输入输出 token、缓存读写 token、输入缓存命中率、截断后的请求和响应体等。Admin UI 会展示这些记录，API 也可以查询：

```bash
curl "http://127.0.0.1:8990/api/admin/call-logs?limit=50&cacheState=hit" \
  -H "x-api-key: sk-admin-your-secret-key"
```

支持的查询参数：`limit`、`offset`、`model`、`cacheState`、`status`。`limit` 最大会被限制为 200。

## 代理池和账号池

代理优先级：

1. 凭据级 `proxyUrl`
2. 全局 `proxyUrl`
3. 无代理

凭据级 `proxyUrl` 设置为 `direct` 时，会显式跳过全局代理。

`proxyPool` 示例：

```json
{
  "proxyPool": [
    {
      "id": "proxy-a",
      "url": "http://127.0.0.1:18080",
      "disabled": false
    },
    {
      "id": "proxy-b",
      "url": "socks5://127.0.0.1:18081",
      "username": "user",
      "password": "pass",
      "disabled": false
    }
  ]
}
```

Admin API 支持：

- `GET /api/admin/credentials`
- `POST /api/admin/credentials`
- `DELETE /api/admin/credentials/:id`
- `POST /api/admin/credentials/:id/disabled`
- `POST /api/admin/credentials/:id/priority`
- `POST /api/admin/credentials/:id/proxy`
- `POST /api/admin/credentials/:id/reset`
- `POST /api/admin/credentials/:id/refresh`
- `GET /api/admin/credentials/:id/balance`
- `GET /api/admin/proxy-pool`
- `POST /api/admin/proxy-pool`
- `POST /api/admin/proxy-pool/assign`
- `POST /api/admin/proxy-pool/:id/disabled`
- `DELETE /api/admin/proxy-pool/:id`
- `GET /api/admin/call-logs`
- `GET /api/admin/config/load-balancing`
- `PUT /api/admin/config/load-balancing`

Admin API 认证支持 `x-api-key: <adminApiKey>` 或 `Authorization: Bearer <adminApiKey>`。

## API 兼容

### Anthropic Messages

- `POST /v1/messages`
- `POST /v1/messages/count_tokens`

### Claude Code 兼容路径

- `POST /cc/v1/messages`
- `POST /cc/v1/messages/count_tokens`

请求使用 `x-api-key` 或 `Authorization: Bearer <apiKey>` 认证。响应保持 Anthropic 兼容结构，支持流式 SSE、thinking、tool use、WebSearch 转换和 usage 字段。

## 模型

模型名会尽量透传到 Anthropic 兼容响应中，并由内部转换为 Kiro 可接受的请求。常见使用：

- `claude-sonnet-4-20250514`
- `claude-opus-4-20250514`
- `claude-haiku-4-5-20251001`

具体可用模型取决于 Kiro 账号能力和上游策略。Free 账号通常不适合走 Opus，项目会根据账号订阅信息做基本判断和故障转移。

## TLS 和代理注意事项

默认 TLS 后端为 `rustls`。如果使用 HTTP/SOCKS5 代理时出现刷新 token 失败、`error sending request`、证书异常等问题，可以尝试切换：

```json
{
  "tlsBackend": "native-tls"
}
```

上游老项目中关于长输出导致 `Write Failed` / 会话卡死的问题仍可能适用，可参考：

- [hank9999/kiro.rs issue #22](https://github.com/hank9999/kiro.rs/issues/22)
- [hank9999/kiro.rs issue #49](https://github.com/hank9999/kiro.rs/issues/49)

## 开源前检查

公开仓库前建议确认：

- `config.json`、`credentials.json`、`credentials.*` 没有被提交。
- `true-cache/`、`input-cache/`、`call-logs/` 没有被提交。
- 日志、截图、README 示例中没有真实账号、token、代理密码、Admin Key。
- README 中的功能描述和当前代码一致，不承诺固定缓存命中率。
- 如果继续沿用上游代码，请保留 LICENSE 和上游来源说明。

## 项目结构

```text
.
├── admin-ui/                 # Admin 前端
├── src/
│   ├── admin/                # Admin API
│   ├── admin_ui/             # 嵌入式 Admin UI 静态资源服务
│   ├── anthropic/            # Anthropic 兼容层、缓存、响应转换
│   ├── kiro/                 # Kiro 请求、token、账号池、上游调用
│   ├── model/                # 配置模型
│   └── call_log.rs           # 调用记录存储
├── config.example.json
├── credentials.example.*.json
├── docker-compose.yml
└── Dockerfile
```

## 技术栈

- Rust / Tokio / Axum
- Reqwest
- Serde
- Tower HTTP
- React / TypeScript / Vite

## License

本项目沿用仓库中的 `LICENSE`。如果基于上游继续分发或开源，请保留原始许可证和上游项目引用。

## 致谢

感谢这些项目和资料对本分支的基础与思路帮助：

- [hank9999/kiro.rs](https://github.com/hank9999/kiro.rs)：本仓库的老项目/上游来源。
- [caidaoli/kiro2api](https://github.com/caidaoli/kiro2api)：同类型 Kiro 转 API 项目参考。
- [Quorinex/Kiro-Go](https://github.com/Quorinex/Kiro-Go)：同类型 Go 实现参考。
- [proxycast](https://github.com/aiclientproxy/proxycast)：代理与转换思路参考。
