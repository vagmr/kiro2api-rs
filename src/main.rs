mod anthropic;
mod http_client;
mod kiro;
mod model;
mod pool;
pub mod token;
mod ui;

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use clap::Parser;
use kiro::model::credentials::KiroCredentials;
use kiro::provider::KiroProvider;
use kiro::token_manager::TokenManager;
use model::arg::Args;
use model::config::Config;
use pool::{Account, AccountPool};

#[tokio::main]
async fn main() {
    // 解析命令行参数
    let args = Args::parse();

    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    // 加载配置
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| Config::default_config_path().to_string());
    let mut config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::warn!("加载配置文件失败: {}, 使用默认配置", e);
        Config::default()
    });

    // 从环境变量覆盖配置
    config.override_from_env();

    // 获取 API Key
    let api_key = config.api_key.clone().unwrap_or_else(|| {
        tracing::error!("配置文件中未设置 apiKey");
        std::process::exit(1);
    });

    // 构建代理配置
    let proxy_config = config.proxy_url.as_ref().map(|url| {
        let mut proxy = http_client::ProxyConfig::new(url);
        if let (Some(username), Some(password)) = (&config.proxy_username, &config.proxy_password) {
            proxy = proxy.with_auth(username, password);
        }
        proxy
    });

    if proxy_config.is_some() {
        tracing::info!("已配置 HTTP 代理: {}", config.proxy_url.as_ref().unwrap());
    }

    // 检查是否启用账号池模式（通过环境变量 POOL_MODE=true）
    let pool_mode = std::env::var("POOL_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let app = if pool_mode {
        tracing::info!("启用账号池模式");
        create_pool_mode_app(&config, &api_key, proxy_config).await
    } else {
        tracing::info!("启用单账号模式");
        create_single_mode_app(&args, &config, &api_key, proxy_config).await
    };

    // 启动服务器
    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("启动 Anthropic API 端点: {}", addr);
    tracing::info!("API Key: {}***", &api_key[..(api_key.len() / 2).min(10)]);
    tracing::info!("可用 API:");
    tracing::info!("  GET  /v1/models");
    tracing::info!("  POST /v1/messages");
    tracing::info!("  POST /v1/messages/count_tokens");
    if pool_mode {
        tracing::info!("管理面板: http://{}/", addr);
    }

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// 创建单账号模式应用
async fn create_single_mode_app(
    args: &Args,
    config: &Config,
    api_key: &str,
    proxy_config: Option<http_client::ProxyConfig>,
) -> Router {
    // 加载凭证（优先环境变量）
    let credentials_path = args
        .credentials
        .clone()
        .unwrap_or_else(|| KiroCredentials::default_credentials_path().to_string());
    let credentials =
        KiroCredentials::load_with_env_fallback(&credentials_path).unwrap_or_else(|e| {
            tracing::error!("加载凭证失败: {}", e);
            tracing::error!(
                "请设置环境变量 (REFRESH_TOKEN, AUTH_METHOD) 或提供 credentials.json 文件"
            );
            std::process::exit(1);
        });

    tracing::debug!("凭证已加载: {:?}", credentials);

    // 创建 KiroProvider
    let token_manager =
        TokenManager::new(config.clone(), credentials.clone(), proxy_config.clone());
    let kiro_provider = KiroProvider::with_proxy(token_manager, proxy_config.clone());

    // 初始化 count_tokens 配置
    token::init_config(token::CountTokensConfig {
        api_url: config.count_tokens_api_url.clone(),
        api_key: config.count_tokens_api_key.clone(),
        auth_type: config.count_tokens_auth_type.clone(),
        proxy: proxy_config,
    });

    // 构建路由
    anthropic::create_router_with_provider(api_key, Some(kiro_provider), credentials.profile_arn)
}

/// 创建账号池模式应用
async fn create_pool_mode_app(
    config: &Config,
    api_key: &str,
    proxy_config: Option<http_client::ProxyConfig>,
) -> Router {
    // 获取数据目录（默认 ./data）
    let data_dir = std::env::var("DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("./data"));

    tracing::info!("数据存储目录: {:?}", data_dir);

    // 创建账号池（带持久化）
    let pool = Arc::new(AccountPool::with_data_dir(
        config.clone(),
        proxy_config.clone(),
        data_dir,
    ));

    // 从文件加载已保存的账号
    if let Err(e) = pool.load_from_file().await {
        tracing::warn!("加载账号文件失败: {}", e);
    }

    // 从文件加载请求记录
    if let Err(e) = pool.load_logs_from_file().await {
        tracing::warn!("加载请求记录失败: {}", e);
    }

    // 从文件加载配额缓存
    if let Err(e) = pool.load_usage_cache().await {
        tracing::warn!("加载配额缓存失败: {}", e);
    }

    // 尝试从环境变量加载初始账号（如果池中没有账号）
    if pool.get_stats().await.total == 0 {
        if let Some(creds) = KiroCredentials::from_env() {
            let account = Account::new(
                uuid::Uuid::new_v4().to_string(),
                "默认账号 (环境变量)",
                creds,
            );
            if let Err(e) = pool.add_account(account).await {
                tracing::warn!("添加默认账号失败: {}", e);
            } else {
                tracing::info!("已从环境变量加载默认账号");
            }
        }
    }

    // 初始化 count_tokens 配置
    token::init_config(token::CountTokensConfig {
        api_url: config.count_tokens_api_url.clone(),
        api_key: config.count_tokens_api_key.clone(),
        auth_type: config.count_tokens_auth_type.clone(),
        proxy: proxy_config,
    });

    // 创建 UI 状态
    let ui_state = ui::UiState {
        pool: pool.clone(),
        start_time: Instant::now(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_key: api_key.to_string(),
    };

    // 构建路由：API + UI
    let api_router = anthropic::create_router_with_pool(api_key, pool);
    let ui_router = ui::create_ui_router(ui_state);

    // 合并路由
    Router::new().merge(api_router).merge(ui_router)
}
