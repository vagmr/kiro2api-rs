//! Kiro OAuth 凭证数据模型
//!
//! 支持从 Kiro IDE 的凭证文件加载，使用 Social 认证方式

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::Path;

/// Kiro OAuth 凭证
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KiroCredentials {
    /// 访问令牌
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,

    /// 刷新令牌
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,

    /// Profile ARN
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,

    /// 过期时间 (RFC3339 格式)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,

    /// 认证方式 (social / idc / builder-id)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,

    /// OIDC Client ID (IdC 认证需要)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    /// OIDC Client Secret (IdC 认证需要)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl KiroCredentials {
    /// 获取默认凭证文件路径
    pub fn default_credentials_path() -> &'static str {
        "credentials.json"
    }

    /// 从环境变量加载凭证
    pub fn from_env() -> Option<Self> {
        let refresh_token = env::var("REFRESH_TOKEN").ok();
        let auth_method = env::var("AUTH_METHOD").ok();

        // 至少需要 refresh_token 和 auth_method
        if refresh_token.is_none() || auth_method.is_none() {
            return None;
        }

        Some(Self {
            access_token: env::var("ACCESS_TOKEN").ok(),
            refresh_token,
            profile_arn: env::var("PROFILE_ARN").ok(),
            expires_at: env::var("EXPIRES_AT")
                .ok()
                .or_else(|| Some("2000-01-01T00:00:00Z".to_string())),
            auth_method,
            client_id: env::var("CLIENT_ID").ok(),
            client_secret: env::var("CLIENT_SECRET").ok(),
        })
    }

    /// 从 JSON 字符串解析凭证
    pub fn from_json(json_string: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_string)
    }

    /// 从文件加载凭证
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path.as_ref())?;
        if content.is_empty() {
            anyhow::bail!("凭证文件为空: {:?}", path.as_ref());
        }
        let credentials = Self::from_json(&content)?;
        Ok(credentials)
    }

    /// 加载凭证：优先从环境变量，其次从文件
    pub fn load_with_env_fallback<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        // 优先尝试从环境变量加载
        if let Some(creds) = Self::from_env() {
            tracing::info!("从环境变量加载凭证");
            return Ok(creds);
        }

        // 回退到文件加载
        Self::load(path)
    }

    /// 序列化为格式化的 JSON 字符串
    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_json() {
        let json = r#"{
            "accessToken": "test_token",
            "refreshToken": "test_refresh",
            "profileArn": "arn:aws:test",
            "expiresAt": "2024-01-01T00:00:00Z",
            "authMethod": "social"
        }"#;

        let creds = KiroCredentials::from_json(json).unwrap();
        assert_eq!(creds.access_token, Some("test_token".to_string()));
        assert_eq!(creds.refresh_token, Some("test_refresh".to_string()));
        assert_eq!(creds.profile_arn, Some("arn:aws:test".to_string()));
        assert_eq!(creds.expires_at, Some("2024-01-01T00:00:00Z".to_string()));
        assert_eq!(creds.auth_method, Some("social".to_string()));
    }

    #[test]
    fn test_from_json_with_unknown_keys() {
        let json = r#"{
            "accessToken": "test_token",
            "unknownField": "should be ignored"
        }"#;

        let creds = KiroCredentials::from_json(json).unwrap();
        assert_eq!(creds.access_token, Some("test_token".to_string()));
    }

    #[test]
    fn test_to_json() {
        let creds = KiroCredentials {
            access_token: Some("token".to_string()),
            refresh_token: None,
            profile_arn: None,
            expires_at: None,
            auth_method: Some("social".to_string()),
            client_id: None,
            client_secret: None,
        };

        let json = creds.to_pretty_json().unwrap();
        assert!(json.contains("accessToken"));
        assert!(json.contains("authMethod"));
        assert!(!json.contains("refreshToken"));
    }

    #[test]
    fn test_default_credentials_path() {
        assert_eq!(
            KiroCredentials::default_credentials_path(),
            "credentials.json"
        );
    }
}
