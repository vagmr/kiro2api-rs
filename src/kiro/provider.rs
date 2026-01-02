//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::http_client::{build_client, ProxyConfig};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::TokenManager;

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 内部使用 Arc<Mutex<_>> 管理 TokenManager 状态，支持线程安全的并发访问
pub struct KiroProvider {
    token_manager: Arc<Mutex<TokenManager>>,
    client: Client,
}

impl KiroProvider {
    /// 创建新的 KiroProvider 实例
    #[allow(dead_code)]
    pub fn new(token_manager: TokenManager) -> Self {
        Self::with_proxy(token_manager, None)
    }

    /// 创建带代理配置的 KiroProvider 实例
    pub fn with_proxy(token_manager: TokenManager, proxy: Option<ProxyConfig>) -> Self {
        let client = build_client(proxy.as_ref(), 720) // 12 分钟超时
            .expect("创建 HTTP 客户端失败");

        Self {
            token_manager: Arc::new(Mutex::new(token_manager)),
            client,
        }
    }

    /// 使用共享的 TokenManager 创建 Provider（适用于账号池模式）
    pub fn with_shared_token_manager(
        token_manager: Arc<Mutex<TokenManager>>,
        proxy: Option<ProxyConfig>,
    ) -> Self {
        let client = build_client(proxy.as_ref(), 720) // 12 分钟超时
            .expect("创建 HTTP 客户端失败");

        Self {
            token_manager,
            client,
        }
    }

    /// 获取 API 基础 URL
    #[allow(dead_code)]
    pub async fn base_url(&self) -> String {
        let region = {
            let tm = self.token_manager.lock().await;
            tm.config().region.clone()
        };
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            region
        )
    }

    /// 获取 API 基础域名
    #[allow(dead_code)]
    pub async fn base_domain(&self) -> String {
        let region = {
            let tm = self.token_manager.lock().await;
            tm.config().region.clone()
        };
        format!("q.{}.amazonaws.com", region)
    }

    /// 构建请求头
    fn build_headers(
        token: &str,
        credentials: &KiroCredentials,
        config: &crate::model::config::Config,
    ) -> anyhow::Result<HeaderMap> {
        let machine_id = machine_id::generate_from_credentials(credentials, config)
            .ok_or_else(|| anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"))?;

        let kiro_version = config.kiro_version.clone();
        let os_name = config.system_version.clone();
        let node_version = config.node_version.clone();
        let base_domain = format!("q.{}.amazonaws.com", config.region);

        let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{}-{}", kiro_version, machine_id);

        let user_agent = format!(
            "aws-sdk-js/1.0.27 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.27 m/E KiroIDE-{}-{}",
            os_name, node_version, kiro_version, machine_id
        );

        let mut headers = HeaderMap::new();

        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-amzn-codewhisperer-optout",
            HeaderValue::from_static("true"),
        );
        headers.insert("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"));
        headers.insert(
            "x-amz-user-agent",
            HeaderValue::from_str(&x_amz_user_agent).unwrap(),
        );
        headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_str(&user_agent).unwrap(),
        );
        headers.insert(HOST, HeaderValue::from_str(&base_domain).unwrap());
        headers.insert(
            "amz-sdk-invocation-id",
            HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap(),
        );
        headers.insert(
            "amz-sdk-request",
            HeaderValue::from_static("attempt=1; max=3"),
        );
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );
        headers.insert(CONNECTION, HeaderValue::from_static("close"));

        Ok(headers)
    }

    async fn acquire_token_snapshot(
        &self,
    ) -> anyhow::Result<(String, crate::model::config::Config, KiroCredentials)> {
        let mut tm = self.token_manager.lock().await;
        let token = tm.ensure_valid_token().await?;
        let config = tm.config().clone();
        let credentials = tm.credentials().clone();
        Ok((token, config, credentials))
    }

    /// 发送非流式 API 请求
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的请求体字符串
    ///
    /// # Returns
    /// 返回原始的 HTTP Response，不做解析
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let (token, config, credentials) = self.acquire_token_snapshot().await?;
        let url = format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            config.region
        );
        let headers = Self::build_headers(&token, &credentials, &config)?;

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .body(request_body.to_string())
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("API 请求失败: {} {}", status, body);
        }

        Ok(response)
    }

    /// 发送流式 API 请求
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的请求体字符串
    ///
    /// # Returns
    /// 返回原始的 HTTP Response，调用方负责处理流式数据
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let (token, config, credentials) = self.acquire_token_snapshot().await?;
        let url = format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            config.region
        );
        let headers = Self::build_headers(&token, &credentials, &config)?;

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .body(request_body.to_string())
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("流式 API 请求失败: {} {}", status, body);
        }

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::model::credentials::KiroCredentials;
    use crate::model::config::Config;

    #[tokio::test]
    async fn test_base_url() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let tm = TokenManager::new(config, credentials, None);
        let provider = KiroProvider::new(tm);
        let url = provider.base_url().await;
        assert!(url.contains("amazonaws.com"));
        assert!(url.contains("generateAssistantResponse"));
    }

    #[tokio::test]
    async fn test_base_domain() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        let credentials = KiroCredentials::default();
        let tm = TokenManager::new(config, credentials, None);
        let provider = KiroProvider::new(tm);
        assert_eq!(provider.base_domain().await, "q.us-east-1.amazonaws.com");
    }

    #[tokio::test]
    async fn test_build_headers() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.8.0".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.profile_arn = Some("arn:aws:sso::123456789:profile/test".to_string());
        credentials.refresh_token = Some("a".repeat(150));

        let headers = KiroProvider::build_headers("test_token", &credentials, &config).unwrap();

        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
        assert_eq!(headers.get("x-amzn-codewhisperer-optout").unwrap(), "true");
        assert_eq!(headers.get("x-amzn-kiro-agent-mode").unwrap(), "vibe");
        assert!(headers
            .get(AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Bearer "));
        assert_eq!(headers.get(CONNECTION).unwrap(), "close");
    }
}
