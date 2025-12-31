//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST};
use reqwest::Client;
use uuid::Uuid;

use crate::http_client::{build_client, ProxyConfig};
use crate::kiro::machine_id;
use crate::kiro::token_manager::TokenManager;

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
pub struct KiroProvider {
    token_manager: TokenManager,
    client: Client,
}

impl KiroProvider {
    /// 创建新的 KiroProvider 实例
    pub fn new(token_manager: TokenManager) -> Self {
        Self::with_proxy(token_manager, None)
    }

    /// 创建带代理配置的 KiroProvider 实例
    pub fn with_proxy(token_manager: TokenManager, proxy: Option<ProxyConfig>) -> Self {
        let client = build_client(proxy.as_ref(), 720) // 12 分钟超时
            .expect("创建 HTTP 客户端失败");

        Self {
            token_manager,
            client,
        }
    }

    /// 获取 API 基础 URL
    pub fn base_url(&self) -> String {
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            self.token_manager.config().region
        )
    }

    /// 获取 API 基础域名
    pub fn base_domain(&self) -> String {
        format!("q.{}.amazonaws.com", self.token_manager.config().region)
    }

    /// 构建请求头
    fn build_headers(&self, token: &str) -> anyhow::Result<HeaderMap> {
        let credentials = self.token_manager.credentials();
        let config = self.token_manager.config();

        let machine_id = machine_id::generate_from_credentials(credentials, config)
            .ok_or_else(|| anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"))?;

        let kiro_version = &config.kiro_version;
        let os_name = &config.system_version;
        let node_version = &config.node_version;

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
        headers.insert(HOST, HeaderValue::from_str(&self.base_domain()).unwrap());
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

    /// 发送非流式 API 请求
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的请求体字符串
    ///
    /// # Returns
    /// 返回原始的 HTTP Response，不做解析
    pub async fn call_api(&mut self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let token = self.token_manager.ensure_valid_token().await?;
        let url = self.base_url();
        let headers = self.build_headers(&token)?;

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
    pub async fn call_api_stream(
        &mut self,
        request_body: &str,
    ) -> anyhow::Result<reqwest::Response> {
        let token = self.token_manager.ensure_valid_token().await?;
        let url = self.base_url();
        let headers = self.build_headers(&token)?;

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

    #[test]
    fn test_base_url() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let tm = TokenManager::new(config, credentials, None);
        let provider = KiroProvider::new(tm);
        assert!(provider.base_url().contains("amazonaws.com"));
        assert!(provider.base_url().contains("generateAssistantResponse"));
    }

    #[test]
    fn test_base_domain() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        let credentials = KiroCredentials::default();
        let tm = TokenManager::new(config, credentials, None);
        let provider = KiroProvider::new(tm);
        assert_eq!(provider.base_domain(), "q.us-east-1.amazonaws.com");
    }

    #[test]
    fn test_build_headers() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.8.0".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.profile_arn = Some("arn:aws:sso::123456789:profile/test".to_string());
        credentials.refresh_token = Some("a".repeat(150));

        let tm = TokenManager::new(config, credentials, None);
        let provider = KiroProvider::new(tm);
        let headers = provider.build_headers("test_token").unwrap();

        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
        assert_eq!(
            headers.get("x-amzn-codewhisperer-optout").unwrap(),
            "true"
        );
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
