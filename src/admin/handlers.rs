//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};

use crate::call_log::CallLogQuery;

use super::{
    middleware::AdminState,
    types::{
        AddCredentialRequest, AddProxyPoolItemRequest, AssignProxyPoolRequest, SetDisabledRequest,
        SetLoadBalancingModeRequest, SetPriorityRequest, SetProxyPoolDisabledRequest,
        SetProxyRequest, SuccessResponse,
    },
};

/// GET /api/admin/credentials
/// 获取所有凭据状态
pub async fn get_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_all_credentials();
    Json(response)
}

/// GET /api/admin/call-logs
/// 获取调用记录
pub async fn get_call_logs(
    State(state): State<AdminState>,
    Query(query): Query<CallLogQuery>,
) -> impl IntoResponse {
    Json(state.service.get_call_logs(query))
}

/// POST /api/admin/credentials/:id/disabled
/// 设置凭据禁用状态
pub async fn set_credential_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    match state.service.set_disabled(id, payload.disabled) {
        Ok(_) => {
            let action = if payload.disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("凭据 #{} 已{}", id, action))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/priority
/// 设置凭据优先级
pub async fn set_credential_priority(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetPriorityRequest>,
) -> impl IntoResponse {
    match state.service.set_priority(id, payload.priority) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 优先级已设置为 {}",
            id, payload.priority
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/proxy
/// 设置凭据级代理
pub async fn set_credential_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetProxyRequest>,
) -> impl IntoResponse {
    match state.service.set_proxy(id, payload) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 代理配置已更新", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/proxy-pool
/// 获取代理池
pub async fn get_proxy_pool(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_proxy_pool())
}

/// POST /api/admin/proxy-pool
/// 添加代理池条目
pub async fn add_proxy_pool_item(
    State(state): State<AdminState>,
    Json(payload): Json<AddProxyPoolItemRequest>,
) -> impl IntoResponse {
    match state.service.add_proxy_pool_item(payload) {
        Ok(_) => Json(SuccessResponse::new("代理已加入代理池")).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/proxy-pool/:id
/// 删除代理池条目
pub async fn delete_proxy_pool_item(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.service.delete_proxy_pool_item(id.clone()) {
        Ok(_) => Json(SuccessResponse::new(format!("代理池条目 {} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxy-pool/:id/disabled
/// 设置代理池条目禁用状态
pub async fn set_proxy_pool_disabled(
    State(state): State<AdminState>,
    Path(id): Path<String>,
    Json(payload): Json<SetProxyPoolDisabledRequest>,
) -> impl IntoResponse {
    let disabled = payload.disabled;
    match state.service.set_proxy_pool_disabled(id.clone(), payload) {
        Ok(_) => {
            let action = if disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!(
                "代理池条目 {} 已{}",
                id, action
            )))
            .into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxy-pool/assign
/// 将代理池分配到账号级代理
pub async fn assign_proxy_pool(
    State(state): State<AdminState>,
    Json(payload): Json<AssignProxyPoolRequest>,
) -> impl IntoResponse {
    match state.service.assign_proxy_pool(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/reset
/// 重置失败计数并重新启用
pub async fn reset_failure_count(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_and_enable(id) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 失败计数已重置并重新启用",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/balance
/// 获取指定凭据的余额
pub async fn get_credential_balance(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_balance(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials
/// 添加新凭据
pub async fn add_credential(
    State(state): State<AdminState>,
    Json(payload): Json<AddCredentialRequest>,
) -> impl IntoResponse {
    match state.service.add_credential(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/credentials/:id
/// 删除凭据
pub async fn delete_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_credential(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/refresh
/// 强制刷新凭据 Token
pub async fn force_refresh_token(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.force_refresh_token(id).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} Token 已强制刷新",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/load-balancing
/// 获取负载均衡模式
pub async fn get_load_balancing_mode(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_load_balancing_mode();
    Json(response)
}

/// PUT /api/admin/config/load-balancing
/// 设置负载均衡模式
pub async fn set_load_balancing_mode(
    State(state): State<AdminState>,
    Json(payload): Json<SetLoadBalancingModeRequest>,
) -> impl IntoResponse {
    match state.service.set_load_balancing_mode(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}
