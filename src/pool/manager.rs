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

/// 账号存储文件名
const ACCOUNTS_FILE: &str = "accounts.json";

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
