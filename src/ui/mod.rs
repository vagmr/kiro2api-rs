//! 管理 UI 模块

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post, delete},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

use crate::kiro::model::credentials::KiroCredentials;
use crate::pool::{Account, AccountPool, SelectionStrategy};

/// UI 共享状态
#[derive(Clone)]
pub struct UiState {
    pub pool: Arc<AccountPool>,
    pub start_time: Instant,
    pub version: String,
    pub api_key: String,
}

/// 创建 UI 路由
pub fn create_ui_router(state: UiState) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/api/status", get(get_status))
        .route("/api/accounts", get(list_accounts))
        .route("/api/accounts", post(add_account))
        .route("/api/accounts/import", post(import_account))
        .route("/api/accounts/{id}", delete(remove_account))
        .route("/api/accounts/{id}/enable", post(enable_account))
        .route("/api/accounts/{id}/disable", post(disable_account))
        .route("/api/strategy", get(get_strategy))
        .route("/api/strategy", post(set_strategy))
        .with_state(state)
}

/// 首页
async fn index_page() -> impl IntoResponse {
    Html(include_str!("index.html"))
}

/// 状态响应
#[derive(Serialize)]
struct StatusResponse {
    status: String,
    version: String,
    uptime_secs: u64,
    pool: crate::pool::PoolStats,
}

/// 获取状态
async fn get_status(State(state): State<UiState>) -> impl IntoResponse {
    let stats = state.pool.get_stats().await;
    Json(StatusResponse {
        status: "running".to_string(),
        version: state.version.clone(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        pool: stats,
    })
}

/// 账号列表响应
#[derive(Serialize)]
struct AccountResponse {
    id: String,
    name: String,
    status: String,
    request_count: u64,
    error_count: u64,
    last_used_at: Option<String>,
    created_at: String,
}

/// 获取账号列表
async fn list_accounts(State(state): State<UiState>) -> impl IntoResponse {
    let accounts = state.pool.list_accounts().await;
    let response: Vec<AccountResponse> = accounts
        .into_iter()
        .map(|a| AccountResponse {
            id: a.id,
            name: a.name,
            status: format!("{:?}", a.status).to_lowercase(),
            request_count: a.request_count,
            error_count: a.error_count,
            last_used_at: a.last_used_at.map(|t| t.to_rfc3339()),
            created_at: a.created_at.to_rfc3339(),
        })
        .collect();
    Json(response)
}

/// 添加账号请求
#[derive(Deserialize)]
struct AddAccountRequest {
    name: String,
    refresh_token: String,
    auth_method: String,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    profile_arn: Option<String>,
}

/// Kiro 原始凭证格式（直接导入）
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroRawCredentials {
    email: Option<String>,
    label: Option<String>,
    provider: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    refresh_token: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

/// 导入账号请求（支持原始 JSON）
#[derive(Deserialize)]
struct ImportAccountRequest {
    /// 原始 JSON 字符串
    raw_json: String,
    /// 可选的自定义名称
    #[serde(default)]
    name: Option<String>,
}

/// 添加账号
async fn add_account(
    State(state): State<UiState>,
    Json(req): Json<AddAccountRequest>,
) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    
    let credentials = KiroCredentials {
        access_token: None,
        refresh_token: Some(req.refresh_token),
        profile_arn: req.profile_arn,
        expires_at: Some("2000-01-01T00:00:00Z".to_string()), // 强制刷新
        auth_method: Some(req.auth_method),
        client_id: req.client_id,
        client_secret: req.client_secret,
    };

    let account = Account::new(&id, req.name, credentials);
    
    match state.pool.add_account(account).await {
        Ok(_) => (StatusCode::CREATED, Json(serde_json::json!({"id": id}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// 导入账号（支持 Kiro 原始 JSON 格式）
async fn import_account(
    State(state): State<UiState>,
    Json(req): Json<ImportAccountRequest>,
) -> impl IntoResponse {
    // 解析原始 JSON
    let raw: KiroRawCredentials = match serde_json::from_str(&req.raw_json) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("JSON 解析失败: {}", e)})),
            );
        }
    };

    let id = uuid::Uuid::new_v4().to_string();
    
    // 自动检测认证方式
    let auth_method = if raw.client_id.is_some() && raw.client_secret.is_some() {
        "idc".to_string()
    } else {
        "social".to_string()
    };

    // 生成名称：优先使用自定义名称，其次 label，再次 email
    let name = req.name
        .or(raw.label.clone())
        .or(raw.email.clone())
        .unwrap_or_else(|| "导入的账号".to_string());

    let credentials = KiroCredentials {
        access_token: raw.access_token,
        refresh_token: Some(raw.refresh_token),
        profile_arn: None,
        expires_at: Some("2000-01-01T00:00:00Z".to_string()),
        auth_method: Some(auth_method),
        client_id: raw.client_id,
        client_secret: raw.client_secret,
    };

    let account = Account::new(&id, name, credentials);
    
    match state.pool.add_account(account).await {
        Ok(_) => (StatusCode::CREATED, Json(serde_json::json!({"id": id}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// 移除账号
async fn remove_account(
    State(state): State<UiState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.pool.remove_account(&id).await {
        Some(_) => StatusCode::NO_CONTENT,
        None => StatusCode::NOT_FOUND,
    }
}

/// 启用账号
async fn enable_account(
    State(state): State<UiState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if state.pool.enable_account(&id).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// 禁用账号
async fn disable_account(
    State(state): State<UiState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if state.pool.disable_account(&id).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// 获取策略
async fn get_strategy(State(state): State<UiState>) -> impl IntoResponse {
    let strategy = state.pool.get_strategy().await;
    Json(serde_json::json!({"strategy": strategy.as_str()}))
}

/// 设置策略请求
#[derive(Deserialize)]
struct SetStrategyRequest {
    strategy: String,
}

/// 设置策略
async fn set_strategy(
    State(state): State<UiState>,
    Json(req): Json<SetStrategyRequest>,
) -> impl IntoResponse {
    let strategy = match req.strategy.as_str() {
        "round-robin" => SelectionStrategy::RoundRobin,
        "random" => SelectionStrategy::Random,
        "least-used" => SelectionStrategy::LeastUsed,
        _ => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "无效的策略"}))),
    };
    state.pool.set_strategy(strategy).await;
    (StatusCode::OK, Json(serde_json::json!({"success": true})))
}
