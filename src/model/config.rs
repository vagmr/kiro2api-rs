use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::Path;

/// KNA 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_region")]
    pub region: String,

    #[serde(default = "default_kiro_version")]
    pub kiro_version: String,

    #[serde(default)]
    pub machine_id: Option<String>,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default = "default_system_version")]
    pub system_version: String,

    #[serde(default = "default_node_version")]
    pub node_version: String,

    /// 外部 count_tokens API 地址（可选）
    #[serde(default)]
    pub count_tokens_api_url: Option<String>,

    /// count_tokens API 密钥（可选）
    #[serde(default)]
    pub count_tokens_api_key: Option<String>,

    /// count_tokens API 认证类型（可选，"x-api-key" 或 "bearer"，默认 "x-api-key"）
    #[serde(default = "default_count_tokens_auth_type")]
    pub count_tokens_auth_type: String,

    /// HTTP 代理地址（可选）
    /// 支持格式: http://host:port, https://host:port, socks5://host:port
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// 代理认证用户名（可选）
    #[serde(default)]
    pub proxy_username: Option<String>,

    /// 代理认证密码（可选）
    #[serde(default)]
    pub proxy_password: Option<String>,
}

impl Config {
    /// 从环境变量覆盖配置
    pub fn override_from_env(&mut self) {
        if let Ok(host) = env::var("HOST") {
            self.host = host;
        }
        if let Ok(port) = env::var("PORT") {
            if let Ok(p) = port.parse() {
                self.port = p;
            }
        }
        if let Ok(region) = env::var("REGION") {
            self.region = region;
        }
        if let Ok(api_key) = env::var("API_KEY") {
            self.api_key = Some(api_key);
        }
        if let Ok(kiro_version) = env::var("KIRO_VERSION") {
            self.kiro_version = kiro_version;
        }
        if let Ok(machine_id) = env::var("MACHINE_ID") {
            self.machine_id = Some(machine_id);
        }
        if let Ok(system_version) = env::var("SYSTEM_VERSION") {
            self.system_version = system_version;
        }
        if let Ok(node_version) = env::var("NODE_VERSION") {
            self.node_version = node_version;
        }
        if let Ok(url) = env::var("COUNT_TOKENS_API_URL") {
            self.count_tokens_api_url = Some(url);
        }
        if let Ok(key) = env::var("COUNT_TOKENS_API_KEY") {
            self.count_tokens_api_key = Some(key);
        }
        if let Ok(auth_type) = env::var("COUNT_TOKENS_AUTH_TYPE") {
            self.count_tokens_auth_type = auth_type;
        }
        if let Ok(proxy) = env::var("PROXY_URL") {
            self.proxy_url = Some(proxy);
        }
        if let Ok(username) = env::var("PROXY_USERNAME") {
            self.proxy_username = Some(username);
        }
        if let Ok(password) = env::var("PROXY_PASSWORD") {
            self.proxy_password = Some(password);
        }
    }
}

fn default_host() -> String {
    env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string())
}

fn default_port() -> u16 {
    env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080)
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_kiro_version() -> String {
    "0.8.0".to_string()
}

fn default_system_version() -> String {
    const SYSTEM_VERSIONS: &[&str] = &["darwin#24.6.0", "win32#10.0.22631"];
    SYSTEM_VERSIONS[fastrand::usize(..SYSTEM_VERSIONS.len())].to_string()
}

fn default_node_version() -> String {
    "22.21.1".to_string()
}

fn default_count_tokens_auth_type() -> String {
    "x-api-key".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            region: default_region(),
            kiro_version: default_kiro_version(),
            machine_id: None,
            api_key: None,
            system_version: default_system_version(),
            node_version: default_node_version(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: default_count_tokens_auth_type(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
        }
    }
}

impl Config {
    /// 获取默认配置文件路径
    pub fn default_config_path() -> &'static str {
        "config.json"
    }

    /// 从文件加载配置
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            // 配置文件不存在，返回默认配置
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }
}
