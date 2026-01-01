//! 账号池管理器

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::http_client::ProxyConfig;
use crate::kiro::token_manager::TokenManager;
use crate::model::config::Config;

use super::account::{Account, AccountStatus};
use super::strategy::SelectionStrategy;
use super::usage::{RequestLog, RequestLogger, RequestStats, UsageLimits};

/// 账号存储文件名
const ACCOUNTS_FILE: &str = "accounts.json";
/// 请求记录存储文件名
const LOGS_FILE: &str = "request_logs.json";
/// 配额缓存存储文件名
const USAGE_CACHE_FILE: &str = "usage_cache.json";

/// 账号池管理器
pub struct AccountPool {
    /// 账号列表
    accounts: RwLock<HashMap<String, Account>>,
    /// Token 管理器缓存
    token_managers: RwLock<HashMap<String, Arc<tokio::sync::Mutex<TokenManager>>>>,
    /// 选择策略
    strategy: RwLock<SelectionStrategy>,
    /// 轮询索引
    round_robin_index: RwLock<usize>,
    /// 全局配置
    config: Config,
    /// 代理配置
    proxy: Option<ProxyConfig>,
    /// 数据存储目录
    data_dir: Option<PathBuf>,
    /// 请求记录器
    request_logger: RwLock<RequestLogger>,
    /// 账号配额缓存
    usage_cache: RwLock<HashMap<String, UsageLimits>>,
}

impl AccountPool {
    /// 创建新的账号池
    pub fn new(config: Config, proxy: Option<ProxyConfig>) -> Self {
        Self {
            accounts: RwLock::new(HashMap::new()),
            token_managers: RwLock::new(HashMap::new()),
            strategy: RwLock::new(SelectionStrategy::default()),
            round_robin_index: RwLock::new(0),
            config,
            proxy,
            data_dir: None,
            request_logger: RwLock::new(RequestLogger::default()),
            usage_cache: RwLock::new(HashMap::new()),
        }
    }

    /// 创建带持久化存储的账号池
    pub fn with_data_dir(config: Config, proxy: Option<ProxyConfig>, data_dir: PathBuf) -> Self {
        Self {
            accounts: RwLock::new(HashMap::new()),
            token_managers: RwLock::new(HashMap::new()),
            strategy: RwLock::new(SelectionStrategy::default()),
            round_robin_index: RwLock::new(0),
            config,
            proxy,
            data_dir: Some(data_dir),
            request_logger: RwLock::new(RequestLogger::default()),
            usage_cache: RwLock::new(HashMap::new()),
        }
    }

    /// 从文件加载账号
    pub async fn load_from_file(&self) -> anyhow::Result<usize> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(0);
        };

        let file_path = data_dir.join(ACCOUNTS_FILE);
        if !file_path.exists() {
            return Ok(0);
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let stored: Vec<StoredAccount> = serde_json::from_str(&content)?;
        
        let mut count = 0;
        for stored_account in stored {
            let account = stored_account.into_account();
            if let Err(e) = self.add_account_internal(account).await {
                tracing::warn!("加载账号失败: {}", e);
            } else {
                count += 1;
            }
        }

        tracing::info!("从文件加载了 {} 个账号", count);
        Ok(count)
    }

    /// 保存账号到文件
    pub async fn save_to_file(&self) -> anyhow::Result<()> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(());
        };

        // 确保目录存在
        tokio::fs::create_dir_all(data_dir).await?;

        let accounts = self.accounts.read().await;
        let stored: Vec<StoredAccount> = accounts
            .values()
            .map(StoredAccount::from_account)
            .collect();

        let content = serde_json::to_string_pretty(&stored)?;
        let file_path = data_dir.join(ACCOUNTS_FILE);
        tokio::fs::write(&file_path, content).await?;

        tracing::debug!("已保存 {} 个账号到文件", stored.len());
        Ok(())
    }

    /// 内部添加账号（不保存文件）
    async fn add_account_internal(&self, account: Account) -> anyhow::Result<()> {
        let id = account.id.clone();
        let credentials = account.credentials.clone();
        
        // 创建 TokenManager
        let token_manager = TokenManager::new(
            self.config.clone(),
            credentials,
            self.proxy.clone(),
        );
        
        let mut accounts = self.accounts.write().await;
        let mut managers = self.token_managers.write().await;
        
        accounts.insert(id.clone(), account);
        managers.insert(id, Arc::new(tokio::sync::Mutex::new(token_manager)));
        
        Ok(())
    }

    /// 添加账号
    pub async fn add_account(&self, account: Account) -> anyhow::Result<()> {
        self.add_account_internal(account).await?;
        self.save_to_file().await?;
        Ok(())
    }

    /// 移除账号
    pub async fn remove_account(&self, id: &str) -> Option<Account> {
        let mut accounts = self.accounts.write().await;
        let mut managers = self.token_managers.write().await;
        
        managers.remove(id);
        let removed = accounts.remove(id);
        
        // 保存到文件
        drop(accounts);
        drop(managers);
        if let Err(e) = self.save_to_file().await {
            tracing::warn!("保存账号文件失败: {}", e);
        }
        
        removed
    }

    /// 获取所有账号（不含凭证）
    pub async fn list_accounts(&self) -> Vec<Account> {
        let accounts = self.accounts.read().await;
        accounts.values().cloned().collect()
    }

    /// 设置选择策略
    pub async fn set_strategy(&self, strategy: SelectionStrategy) {
        *self.strategy.write().await = strategy;
    }

    /// 获取当前策略
    pub async fn get_strategy(&self) -> SelectionStrategy {
        *self.strategy.read().await
    }

    /// 选择一个可用账号并获取其 TokenManager
    pub async fn select_account(&self) -> Option<(String, Arc<tokio::sync::Mutex<TokenManager>>)> {
        let strategy = *self.strategy.read().await;
        let mut accounts = self.accounts.write().await;
        
        // 获取可用账号列表
        let available: Vec<&str> = accounts
            .iter()
            .filter(|(_, a)| a.is_available())
            .map(|(id, _)| id.as_str())
            .collect();
        
        if available.is_empty() {
            return None;
        }

        // 根据策略选择
        let selected_id = match strategy {
            SelectionStrategy::RoundRobin => {
                let mut index = self.round_robin_index.write().await;
                let id = available[*index % available.len()].to_string();
                *index = (*index + 1) % available.len();
                id
            }
            SelectionStrategy::Random => {
                let idx = fastrand::usize(..available.len());
                available[idx].to_string()
            }
            SelectionStrategy::LeastUsed => {
                available
                    .iter()
                    .min_by_key(|id| accounts.get(**id).map(|a| a.request_count).unwrap_or(u64::MAX))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| available[0].to_string())
            }
        };

        // 记录使用
        if let Some(account) = accounts.get_mut(&selected_id) {
            account.record_use();
        }

        // 获取 TokenManager
        let managers = self.token_managers.read().await;
        managers.get(&selected_id).map(|tm| (selected_id, tm.clone()))
    }

    /// 启用账号
    pub async fn enable_account(&self, id: &str) -> bool {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.enable();
            drop(accounts);
            let _ = self.save_to_file().await;
            true
        } else {
            false
        }
    }

    /// 禁用账号
    pub async fn disable_account(&self, id: &str) -> bool {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.disable();
            drop(accounts);
            let _ = self.save_to_file().await;
            true
        } else {
            false
        }
    }

    /// 记录账号错误
    pub async fn record_error(&self, id: &str, is_rate_limit: bool) {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.record_error(is_rate_limit);
            tracing::info!(
                "账号 {} 记录错误，限流: {}，当前错误数: {}，状态: {:?}",
                id, is_rate_limit, account.error_count, account.status
            );
            drop(accounts);
            let _ = self.save_to_file().await;
        }
    }

    /// 标记账号为失效
    pub async fn mark_invalid(&self, id: &str) {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.mark_invalid();
            tracing::warn!(
                "账号 {} 已标记为失效，错误数: {}",
                id, account.error_count
            );
            drop(accounts);
            let _ = self.save_to_file().await;
        }
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> PoolStats {
        let accounts = self.accounts.read().await;
        
        let total = accounts.len();
        let active = accounts.values().filter(|a| a.status == AccountStatus::Active).count();
        let cooldown = accounts.values().filter(|a| a.status == AccountStatus::Cooldown).count();
        let invalid = accounts.values().filter(|a| a.status == AccountStatus::Invalid).count();
        let disabled = accounts.values().filter(|a| a.status == AccountStatus::Disabled).count();
        let total_requests: u64 = accounts.values().map(|a| a.request_count).sum();
        let total_errors: u64 = accounts.values().map(|a| a.error_count).sum();

        PoolStats {
            total,
            active,
            cooldown,
            invalid,
            disabled,
            total_requests,
            total_errors,
        }
    }

    /// 添加请求记录
    pub async fn add_request_log(&self, log: RequestLog) {
        let mut logger = self.request_logger.write().await;
        logger.add(log);
        
        // 异步保存到文件（不阻塞）
        if let Some(data_dir) = &self.data_dir {
            let logs = logger.get_all();
            let file_path = data_dir.join(LOGS_FILE);
            tokio::spawn(async move {
                if let Ok(content) = serde_json::to_string(&logs) {
                    let _ = tokio::fs::write(&file_path, content).await;
                }
            });
        }
    }

    /// 获取最近的请求记录
    pub async fn get_recent_logs(&self, n: usize) -> Vec<RequestLog> {
        let logger = self.request_logger.read().await;
        logger.get_recent(n)
    }

    /// 获取请求统计
    pub async fn get_request_stats(&self) -> RequestStats {
        let logger = self.request_logger.read().await;
        logger.get_stats()
    }

    /// 从文件加载请求记录
    pub async fn load_logs_from_file(&self) -> anyhow::Result<usize> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(0);
        };

        let file_path = data_dir.join(LOGS_FILE);
        if !file_path.exists() {
            return Ok(0);
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let mut logs: Vec<RequestLog> = serde_json::from_str(&content)?;
        
        // 只保留最新的 1000 条（如果超过的话）
        if logs.len() > 1000 {
            logs = logs.split_off(logs.len() - 1000);
        }
        
        let count = logs.len();
        let mut logger = self.request_logger.write().await;
        for log in logs {
            logger.add(log);
        }

        tracing::info!("从文件加载了 {} 条请求记录", count);
        Ok(count)
    }

    /// 获取账号配额（带缓存）
    pub async fn get_account_usage(&self, id: &str) -> Option<UsageLimits> {
        let cache = self.usage_cache.read().await;
        cache.get(id).cloned()
    }

    /// 刷新账号配额
    pub async fn refresh_account_usage(&self, id: &str) -> anyhow::Result<UsageLimits> {
        // 获取 TokenManager
        let managers = self.token_managers.read().await;
        let tm = managers.get(id).ok_or_else(|| anyhow::anyhow!("账号不存在"))?;
        
        // 获取 access_token
        let mut tm_guard = tm.lock().await;
        let token = match tm_guard.ensure_valid_token().await {
            Ok(t) => t,
            Err(e) => {
                let error_msg = e.to_string();
                // 检测 403/suspended 错误，自动禁用账号
                if error_msg.contains("403") || error_msg.contains("suspended") || error_msg.contains("SUSPENDED") {
                    drop(tm_guard);
                    drop(managers);
                    self.mark_invalid(id).await;
                    tracing::warn!("账号 {} 获取 token 失败，已标记为失效: {}", id, error_msg);
                }
                return Err(e);
            }
        };
        drop(tm_guard);
        drop(managers);
        
        // 调用 API 获取配额
        let usage = match super::usage::check_usage_limits(&token).await {
            Ok(u) => u,
            Err(e) => {
                let error_msg = e.to_string();
                // 检测 403/suspended 错误，自动禁用账号
                if error_msg.contains("403") || error_msg.contains("suspended") || error_msg.contains("SUSPENDED") {
                    self.mark_invalid(id).await;
                    tracing::warn!("账号 {} 获取配额失败，已标记为失效: {}", id, error_msg);
                }
                return Err(e);
            }
        };
        
        // 更新缓存
        let mut cache = self.usage_cache.write().await;
        cache.insert(id.to_string(), usage.clone());
        drop(cache);
        
        // 保存到文件
        self.save_usage_cache().await;
        
        Ok(usage)
    }

    /// 保存配额缓存到文件
    async fn save_usage_cache(&self) {
        if let Some(data_dir) = &self.data_dir {
            let cache = self.usage_cache.read().await;
            let file_path = data_dir.join(USAGE_CACHE_FILE);
            if let Ok(content) = serde_json::to_string(&*cache) {
                let _ = tokio::fs::write(&file_path, content).await;
            }
        }
    }

    /// 从文件加载配额缓存
    pub async fn load_usage_cache(&self) -> anyhow::Result<usize> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(0);
        };

        let file_path = data_dir.join(USAGE_CACHE_FILE);
        if !file_path.exists() {
            return Ok(0);
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let loaded: HashMap<String, UsageLimits> = serde_json::from_str(&content)?;
        
        let count = loaded.len();
        let mut cache = self.usage_cache.write().await;
        *cache = loaded;

        tracing::info!("从文件加载了 {} 个配额缓存", count);
        Ok(count)
    }

    /// 刷新所有账号配额
    pub async fn refresh_all_usage(&self) -> Vec<(String, Result<UsageLimits, String>)> {
        let accounts = self.accounts.read().await;
        let ids: Vec<String> = accounts.keys().cloned().collect();
        drop(accounts);

        let mut results = Vec::new();
        for id in ids {
            let result = match self.refresh_account_usage(&id).await {
                Ok(usage) => Ok(usage),
                Err(e) => Err(e.to_string()),
            };
            results.push((id, result));
        }
        results
    }

    /// 获取所有账号配额缓存
    pub async fn get_all_usage(&self) -> HashMap<String, UsageLimits> {
        let cache = self.usage_cache.read().await;
        cache.clone()
    }
}

/// 账号池统计
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolStats {
    pub total: usize,
    pub active: usize,
    pub cooldown: usize,
    pub invalid: usize,
    pub disabled: usize,
    pub total_requests: u64,
    pub total_errors: u64,
}

/// 用于持久化存储的账号结构
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StoredAccount {
    id: String,
    name: String,
    status: super::account::AccountStatus,
    request_count: u64,
    error_count: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    // 凭证信息
    refresh_token: Option<String>,
    auth_method: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    profile_arn: Option<String>,
}

impl StoredAccount {
    fn from_account(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            name: account.name.clone(),
            status: account.status,
            request_count: account.request_count,
            error_count: account.error_count,
            created_at: account.created_at,
            refresh_token: account.credentials.refresh_token.clone(),
            auth_method: account.credentials.auth_method.clone(),
            client_id: account.credentials.client_id.clone(),
            client_secret: account.credentials.client_secret.clone(),
            profile_arn: account.credentials.profile_arn.clone(),
        }
    }

    fn into_account(self) -> Account {
        use crate::kiro::model::credentials::KiroCredentials;
        
        let credentials = KiroCredentials {
            access_token: None,
            refresh_token: self.refresh_token,
            profile_arn: self.profile_arn,
            expires_at: Some("2000-01-01T00:00:00Z".to_string()),
            auth_method: self.auth_method,
            client_id: self.client_id,
            client_secret: self.client_secret,
        };

        Account {
            id: self.id,
            name: self.name,
            credentials,
            status: self.status,
            request_count: self.request_count,
            error_count: self.error_count,
            last_used_at: None,
            cooldown_until: None,
            created_at: self.created_at,
        }
    }
}
