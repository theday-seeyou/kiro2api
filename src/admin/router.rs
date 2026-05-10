//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post},
};

use super::{
    handlers::{
        add_credential, add_proxy_pool_item, assign_proxy_pool, delete_credential,
        delete_proxy_pool_item, force_refresh_token, get_all_credentials, get_call_logs,
        get_credential_balance, get_load_balancing_mode, get_proxy_pool, reset_failure_count,
        set_credential_disabled, set_credential_priority, set_credential_proxy,
        set_load_balancing_mode, set_proxy_pool_disabled,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 端点
/// - `GET /credentials` - 获取所有凭据状态
/// - `POST /credentials` - 添加新凭据
/// - `DELETE /credentials/:id` - 删除凭据
/// - `POST /credentials/:id/disabled` - 设置凭据禁用状态
/// - `POST /credentials/:id/priority` - 设置凭据优先级
/// - `POST /credentials/:id/proxy` - 设置凭据级代理
/// - `POST /credentials/:id/reset` - 重置失败计数
/// - `POST /credentials/:id/refresh` - 强制刷新 Token
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /proxy-pool` - 获取代理池
/// - `POST /proxy-pool` - 添加代理池条目
/// - `POST /proxy-pool/assign` - 将代理池分配到账号级代理
/// - `POST /proxy-pool/:id/disabled` - 设置代理池条目禁用状态
/// - `DELETE /proxy-pool/:id` - 删除代理池条目
/// - `GET /call-logs` - 获取调用记录
/// - `GET /config/load-balancing` - 获取负载均衡模式
/// - `PUT /config/load-balancing` - 设置负载均衡模式
///
/// # 认证
/// 需要 Admin API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn create_admin_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/proxy", post(set_credential_proxy))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/proxy-pool", get(get_proxy_pool).post(add_proxy_pool_item))
        .route("/proxy-pool/assign", post(assign_proxy_pool))
        .route("/proxy-pool/{id}", delete(delete_proxy_pool_item))
        .route("/proxy-pool/{id}/disabled", post(set_proxy_pool_disabled))
        .route("/call-logs", get(get_call_logs))
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
