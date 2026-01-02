//! 使用量和配额管理模块

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// 请求记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLog {
    /// 请求 ID
    pub id: String,
    /// 账号 ID
    pub account_id: String,
    /// 账号名称
    pub account_name: String,
    /// 模型
    pub model: String,
    /// 输入 tokens
    pub input_tokens: i32,
    /// 输出 tokens
    pub output_tokens: i32,
    /// 是否成功
    pub success: bool,
    /// 错误信息
    pub error: Option<String>,
    /// 请求时间
    pub timestamp: DateTime<Utc>,
    /// 耗时（毫秒）
    pub duration_ms: u64,
}

/// 使用限制信息（来自 AWS API）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimits {
    /// 资源类型
    pub resource_type: String,
    /// 使用限额
    pub usage_limit: f64,
    /// 当前使用量
    pub current_usage: f64,
    /// 剩余可用
    pub available: f64,
    /// 重置日期
    pub next_reset: Option<DateTime<Utc>>,
    /// 免费试用信息
    pub free_trial: Option<FreeTrialInfo>,
    /// 用户邮箱
    pub user_email: Option<String>,
    /// 订阅类型
    pub subscription_type: Option<String>,
}

/// 免费试用信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreeTrialInfo {
    pub status: String,
    pub usage_limit: f64,
    pub current_usage: f64,
    pub expiry: Option<DateTime<Utc>>,
}

/// 请求记录管理器
pub struct RequestLogger {
    /// 请求记录（最近 N 条）
    logs: VecDeque<RequestLog>,
    /// 最大记录数
    max_logs: usize,
}

impl RequestLogger {
    /// 创建新的请求记录器
    pub fn new(max_logs: usize) -> Self {
        Self {
            logs: VecDeque::with_capacity(max_logs),
            max_logs,
        }
    }

    /// 添加请求记录
    pub fn add(&mut self, log: RequestLog) {
        if self.logs.len() >= self.max_logs {
            self.logs.pop_front();
        }
        self.logs.push_back(log);
    }

    /// 获取所有记录
    pub fn get_all(&self) -> Vec<RequestLog> {
        self.logs.iter().cloned().collect()
    }

    /// 获取最近 N 条记录
    pub fn get_recent(&self, n: usize) -> Vec<RequestLog> {
        self.logs.iter().rev().take(n).cloned().collect()
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> RequestStats {
        let total = self.logs.len();
        let success = self.logs.iter().filter(|l| l.success).count();
        let failed = total - success;
        let total_input_tokens: i64 = self.logs.iter().map(|l| l.input_tokens as i64).sum();
        // 忽略 -1（流式请求无法统计）
        let total_output_tokens: i64 = self
            .logs
            .iter()
            .filter(|l| l.output_tokens >= 0)
            .map(|l| l.output_tokens as i64)
            .sum();
        let avg_duration = if total > 0 {
            self.logs.iter().map(|l| l.duration_ms).sum::<u64>() / total as u64
        } else {
            0
        };

        RequestStats {
            total_requests: total,
            success_requests: success,
            failed_requests: failed,
            total_input_tokens,
            total_output_tokens,
            avg_duration_ms: avg_duration,
        }
    }
}

/// 请求统计
#[derive(Debug, Clone, Serialize)]
pub struct RequestStats {
    pub total_requests: usize,
    pub success_requests: usize,
    pub failed_requests: usize,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub avg_duration_ms: u64,
}

impl Default for RequestLogger {
    fn default() -> Self {
        Self::new(1000) // 默认保留 1000 条记录
    }
}

/// AWS 使用限制 API 响应结构
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsUsageLimitsResponse {
    pub usage_breakdown_list: Vec<AwsUsageBreakdown>,
    pub user_info: Option<AwsUserInfo>,
    pub subscription_info: Option<AwsSubscriptionInfo>,
    pub next_date_reset: Option<f64>,
    pub days_until_reset: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsUsageBreakdown {
    pub resource_type: String,
    pub usage_limit: Option<i32>,
    pub usage_limit_with_precision: Option<f64>,
    pub current_usage: Option<i32>,
    pub current_usage_with_precision: Option<f64>,
    pub free_trial_info: Option<AwsFreeTrialInfo>,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsFreeTrialInfo {
    pub free_trial_status: String,
    pub usage_limit: Option<i32>,
    pub usage_limit_with_precision: Option<f64>,
    pub current_usage: Option<i32>,
    pub current_usage_with_precision: Option<f64>,
    pub free_trial_expiry: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsUserInfo {
    pub email: Option<String>,
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsSubscriptionInfo {
    #[serde(rename = "type")]
    pub subscription_type: Option<String>,
    pub subscription_title: Option<String>,
}

/// 检查账号使用限制
pub async fn check_usage_limits(access_token: &str) -> anyhow::Result<UsageLimits> {
    let client = reqwest::Client::new();

    let url = "https://codewhisperer.us-east-1.amazonaws.com/getUsageLimits?isEmailRequired=true&origin=AI_EDITOR&resourceType=AGENTIC_REQUEST";

    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("x-amz-user-agent", "aws-sdk-js/1.0.0 KiroIDE")
        .header("user-agent", "aws-sdk-js/1.0.0 KiroIDE")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("获取使用限制失败: {} - {}", status, body);
    }

    let aws_response: AwsUsageLimitsResponse = response.json().await?;

    // 解析 CREDIT 类型的使用限制
    for breakdown in &aws_response.usage_breakdown_list {
        if breakdown.resource_type == "CREDIT" {
            let mut total_limit = breakdown.usage_limit_with_precision.unwrap_or(0.0);
            let mut total_used = breakdown.current_usage_with_precision.unwrap_or(0.0);

            let free_trial = if let Some(ft) = &breakdown.free_trial_info {
                if ft.free_trial_status == "ACTIVE" {
                    let ft_limit = ft.usage_limit_with_precision.unwrap_or(0.0);
                    let ft_used = ft.current_usage_with_precision.unwrap_or(0.0);
                    total_limit += ft_limit;
                    total_used += ft_used;

                    Some(FreeTrialInfo {
                        status: ft.free_trial_status.clone(),
                        usage_limit: ft_limit,
                        current_usage: ft_used,
                        expiry: ft.free_trial_expiry.map(|ts| {
                            DateTime::from_timestamp_millis(ts as i64).unwrap_or_default()
                        }),
                    })
                } else {
                    None
                }
            } else {
                None
            };

            let next_reset = aws_response
                .next_date_reset
                .map(|ts| DateTime::from_timestamp_millis(ts as i64).unwrap_or_default());

            return Ok(UsageLimits {
                resource_type: "CREDIT".to_string(),
                usage_limit: total_limit,
                current_usage: total_used,
                available: (total_limit - total_used).max(0.0),
                next_reset,
                free_trial,
                user_email: aws_response
                    .user_info
                    .as_ref()
                    .and_then(|u| u.email.clone()),
                subscription_type: aws_response
                    .subscription_info
                    .as_ref()
                    .and_then(|s| s.subscription_type.clone()),
            });
        }
    }

    anyhow::bail!("未找到 CREDIT 类型的使用限制")
}
