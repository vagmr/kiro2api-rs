//! Anthropic API Handler 函数

use std::convert::Infallible;

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    Json as JsonExtractor,
};
use bytes::Bytes;
use futures::{stream, Stream, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::time::interval;
use uuid::Uuid;
use crate::token;
use crate::kiro::model::events::Event;
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::parser::decoder::EventStreamDecoder;

use super::converter::{convert_request, ConversionError};
use super::middleware::AppState;
use super::stream::{SseEvent, StreamContext};
use super::types::{
    CountTokensRequest, CountTokensResponse, ErrorResponse, MessagesRequest, Model, ModelsResponse,
};

/// GET /v1/models
///
/// 返回可用的模型列表
pub async fn get_models() -> impl IntoResponse {
    tracing::info!("Received GET /v1/models request");

    let models = vec![
        Model {
            id: "claude-sonnet-4-5-20250929".to_string(),
            object: "model".to_string(),
            created: 1727568000,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-opus-4-5-20251101".to_string(),
            object: "model".to_string(),
            created: 1730419200,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-haiku-4-5-20251001".to_string(),
            object: "model".to_string(),
            created: 1727740800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
    ];

    Json(ModelsResponse {
        object: "list".to_string(),
        data: models,
    })
}

/// POST /v1/messages
///
/// 创建消息（对话）
pub async fn post_messages(
    State(state): State<AppState>,
    JsonExtractor(payload): JsonExtractor<MessagesRequest>,
) -> Response {
    let start_time = std::time::Instant::now();
    
    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        "Received POST /v1/messages request"
    );

    // 获取 provider：优先从账号池获取，否则使用单账号模式
    let (provider, account_id, account_name, pool_ref) = if let Some(pool) = &state.account_pool {
        match pool.select_account().await {
            Some((id, tm)) => {
                // 获取账号名称
                let acc_name = pool.list_accounts().await
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| "未知账号".to_string());
                
                // 从 TokenManager 创建临时 KiroProvider
                let tm_guard = tm.lock().await;
                let provider = crate::kiro::provider::KiroProvider::with_proxy(
                    crate::kiro::token_manager::TokenManager::new(
                        tm_guard.config().clone(),
                        tm_guard.credentials().clone(),
                        None, // TODO: 支持代理
                    ),
                    None,
                );
                drop(tm_guard);
                (std::sync::Arc::new(tokio::sync::Mutex::new(provider)), Some(id), acc_name, Some(pool.clone()))
            }
            None => {
                tracing::error!("账号池中没有可用账号");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ErrorResponse::new(
                        "service_unavailable",
                        "No available accounts in pool",
                    )),
                )
                    .into_response();
            }
        }
    } else {
        // 单账号模式
        match &state.kiro_provider {
            Some(p) => (p.clone(), None, "单账号模式".to_string(), None),
            None => {
                tracing::error!("KiroProvider 未配置");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ErrorResponse::new(
                        "service_unavailable",
                        "Kiro API provider not configured",
                    )),
                )
                    .into_response();
            }
        }
    };

    // 获取 profile_arn
    let profile_arn = if account_id.is_some() {
        // 从账号池获取时，从 TokenManager 获取 profile_arn
        let provider_guard = provider.lock().await;
        // 暂时使用 state 中的 profile_arn
        drop(provider_guard);
        state.profile_arn.clone()
    } else {
        state.profile_arn.clone()
    };

    // 转换请求
    let conversion_result = match convert_request(&payload) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 构建 Kiro 请求
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: profile_arn.clone(),
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    tracing::debug!("Kiro request body: {}", request_body);

    // 估算输入 tokens
    let input_tokens = token::count_all_tokens(payload.model.clone(), payload.system, payload.messages, payload.tools) as i32;

    // 检查是否启用了thinking
    let thinking_enabled = payload.thinking
        .as_ref()
        .map(|t| t.thinking_type == "enabled")
        .unwrap_or(false);

    if payload.stream {
        // 流式响应
        handle_stream_request(provider, &request_body, &payload.model, input_tokens, thinking_enabled, account_id, account_name, pool_ref, start_time).await
    } else {
        // 非流式响应
        handle_non_stream_request(provider, &request_body, &payload.model, input_tokens, account_id, account_name, pool_ref, start_time).await
    }
}

/// 处理流式请求
async fn handle_stream_request(
    provider: std::sync::Arc<tokio::sync::Mutex<crate::kiro::provider::KiroProvider>>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    account_id: Option<String>,
    account_name: String,
    pool: Option<std::sync::Arc<crate::pool::AccountPool>>,
    start_time: std::time::Instant,
) -> Response {
    // 调用 Kiro API
    let response = {
        let mut provider_guard = provider.lock().await;
        match provider_guard.call_api_stream(request_body).await {
            Ok(resp) => resp,
            Err(e) => {
                let error_msg = e.to_string();
                tracing::error!("Kiro API 调用失败: {}", error_msg);
                
                // 记录错误到账号池
                if let (Some(id), Some(pool)) = (&account_id, &pool) {
                    let is_rate_limit = error_msg.contains("429") || error_msg.contains("rate");
                    let is_suspended = error_msg.contains("suspended") || error_msg.contains("403");
                    
                    if is_suspended {
                        pool.mark_invalid(id).await;
                        tracing::warn!("账号 {} 已被标记为失效（暂停）", id);
                    } else {
                        pool.record_error(id, is_rate_limit).await;
                        tracing::warn!("账号 {} 记录错误，限流: {}", id, is_rate_limit);
                    }
                    
                    // 记录失败的请求
                    let log = crate::pool::RequestLog {
                        id: uuid::Uuid::new_v4().to_string(),
                        account_id: id.clone(),
                        account_name: account_name.clone(),
                        model: model.to_string(),
                        input_tokens,
                        output_tokens: 0,
                        success: false,
                        error: Some(error_msg.clone()),
                        timestamp: chrono::Utc::now(),
                        duration_ms: start_time.elapsed().as_millis() as u64,
                    };
                    pool.add_request_log(log).await;
                }
                
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse::new(
                        "api_error",
                        format!("上游 API 调用失败: {}", e),
                    )),
                )
                    .into_response();
            }
        }
    };

    // 记录成功的流式请求（输出 tokens 设为 -1 表示流式无法统计）
    if let (Some(id), Some(pool)) = (&account_id, &pool) {
        let log = crate::pool::RequestLog {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: id.clone(),
            account_name,
            model: model.to_string(),
            input_tokens,
            output_tokens: -1, // 流式请求无法统计输出 tokens
            success: true,
            error: None,
            timestamp: chrono::Utc::now(),
            duration_ms: start_time.elapsed().as_millis() as u64,
        };
        pool.add_request_log(log).await;
    }

    // 创建流处理上下文
    let mut ctx = StreamContext::new_with_thinking(model, input_tokens, thinking_enabled);

    // 生成初始事件
    let initial_events = ctx.generate_initial_events();

    // 创建 SSE 流
    let stream = create_sse_stream(response, ctx, initial_events);

    // 返回 SSE 响应
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Ping 事件间隔（25秒）
const PING_INTERVAL_SECS: u64 = 25;

/// 创建 ping 事件的 SSE 字符串
fn create_ping_sse() -> Bytes {
    Bytes::from("event: ping\ndata: {\"type\": \"ping\"}\n\n")
}

/// 创建 SSE 事件流
fn create_sse_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    initial_events: Vec<SseEvent>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    // 先发送初始事件
    let initial_stream = stream::iter(
        initial_events
            .into_iter()
            .map(|e| Ok(Bytes::from(e.to_sse_string()))),
    );

    // 然后处理 Kiro 响应流，同时每25秒发送 ping 保活
    let body_stream = response.bytes_stream();

    let processing_stream = stream::unfold(
        (body_stream, ctx, EventStreamDecoder::new(), false, interval(Duration::from_secs(PING_INTERVAL_SECS))),
        |(mut body_stream, mut ctx, mut decoder, finished, mut ping_interval)| async move {
            if finished {
                return None;
            }

            // 使用 select! 同时等待数据和 ping 定时器
            tokio::select! {
                // 处理数据流
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            // 解码事件
                            if let Err(e) = decoder.feed(&chunk) {
                                tracing::warn!("缓冲区溢出: {}", e);
                            }

                            let mut events = Vec::new();
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => {
                                        if let Ok(event) = Event::from_frame(frame) {
                                            let sse_events = ctx.process_kiro_event(&event);
                                            events.extend(sse_events);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("解码事件失败: {}", e);
                                    }
                                }
                            }

                            // 转换为 SSE 字节流
                            let bytes: Vec<Result<Bytes, Infallible>> = events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();

                            Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval)))
                        }
                        Some(Err(e)) => {
                            tracing::error!("读取响应流失败: {}", e);
                            // 发送最终事件并结束
                            let final_events = ctx.generate_final_events();
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)))
                        }
                        None => {
                            // 流结束，发送最终事件
                            let final_events = ctx.generate_final_events();
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)))
                        }
                    }
                }
                // 发送 ping 保活
                _ = ping_interval.tick() => {
                    tracing::trace!("发送 ping 保活事件");
                    let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                    Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval)))
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

/// 上下文窗口大小（200k tokens）
const CONTEXT_WINDOW_SIZE: i32 = 200_000;

/// 处理非流式请求
async fn handle_non_stream_request(
    provider: std::sync::Arc<tokio::sync::Mutex<crate::kiro::provider::KiroProvider>>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    account_id: Option<String>,
    account_name: String,
    pool: Option<std::sync::Arc<crate::pool::AccountPool>>,
    start_time: std::time::Instant,
) -> Response {
    // 调用 Kiro API
    let response = {
        let mut provider_guard = provider.lock().await;
        match provider_guard.call_api(request_body).await {
            Ok(resp) => resp,
            Err(e) => {
                let error_msg = e.to_string();
                tracing::error!("Kiro API 调用失败: {}", error_msg);
                
                // 记录错误到账号池
                if let (Some(id), Some(pool)) = (&account_id, &pool) {
                    let is_rate_limit = error_msg.contains("429") || error_msg.contains("rate");
                    let is_suspended = error_msg.contains("suspended") || error_msg.contains("403");
                    
                    if is_suspended {
                        pool.mark_invalid(id).await;
                        tracing::warn!("账号 {} 已被标记为失效（暂停）", id);
                    } else {
                        pool.record_error(id, is_rate_limit).await;
                        tracing::warn!("账号 {} 记录错误，限流: {}", id, is_rate_limit);
                    }
                    
                    // 记录失败的请求
                    let log = crate::pool::RequestLog {
                        id: uuid::Uuid::new_v4().to_string(),
                        account_id: id.clone(),
                        account_name: account_name.clone(),
                        model: model.to_string(),
                        input_tokens,
                        output_tokens: 0,
                        success: false,
                        error: Some(error_msg.clone()),
                        timestamp: chrono::Utc::now(),
                        duration_ms: start_time.elapsed().as_millis() as u64,
                    };
                    pool.add_request_log(log).await;
                }
                
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse::new(
                        "api_error",
                        format!("上游 API 调用失败: {}", e),
                    )),
                )
                    .into_response();
            }
        }
    };

    // 读取响应体
    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("读取响应体失败: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    format!("读取响应失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    // 解析事件流
    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&body_bytes) {
        tracing::warn!("缓冲区溢出: {}", e);
    }

    let mut text_content = String::new();
    let mut tool_uses: Vec<serde_json::Value> = Vec::new();
    let mut has_tool_use = false;
    let mut stop_reason = "end_turn".to_string();
    // 从 contextUsageEvent 计算的实际输入 tokens
    let mut context_input_tokens: Option<i32> = None;

    // 收集工具调用的增量 JSON
    let mut tool_json_buffers: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => {
                if let Ok(event) = Event::from_frame(frame) {
                    match event {
                        Event::AssistantResponse(resp) => {
                            text_content.push_str(&resp.content);
                        }
                        Event::ToolUse(tool_use) => {
                            has_tool_use = true;

                            // 累积工具的 JSON 输入
                            let buffer = tool_json_buffers
                                .entry(tool_use.tool_use_id.clone())
                                .or_insert_with(String::new);
                            buffer.push_str(&tool_use.input);

                            // 如果是完整的工具调用，添加到列表
                            if tool_use.stop {
                                let input: serde_json::Value = serde_json::from_str(buffer)
                                    .unwrap_or_else(|e| {
                                        tracing::warn!(
                                            "工具输入 JSON 解析失败: {}, tool_use_id: {}, 原始内容: {}",
                                            e, tool_use.tool_use_id, buffer
                                        );
                                        serde_json::json!({})
                                    });

                                tool_uses.push(json!({
                                    "type": "tool_use",
                                    "id": tool_use.tool_use_id,
                                    "name": tool_use.name,
                                    "input": input
                                }));
                            }
                        }
                        Event::ContextUsage(context_usage) => {
                            // 从上下文使用百分比计算实际的 input_tokens
                            // 公式: percentage * 200000 / 100 = percentage * 2000
                            let actual_input_tokens = (context_usage.context_usage_percentage * (CONTEXT_WINDOW_SIZE as f64) / 100.0) as i32;
                            context_input_tokens = Some(actual_input_tokens);
                            tracing::debug!(
                                "收到 contextUsageEvent: {}%, 计算 input_tokens: {}",
                                context_usage.context_usage_percentage,
                                actual_input_tokens
                            );
                        }
                        Event::Exception { exception_type, .. } => {
                            if exception_type == "ContentLengthExceededException" {
                                stop_reason = "max_tokens".to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                tracing::warn!("解码事件失败: {}", e);
            }
        }
    }

    // 确定 stop_reason
    if has_tool_use && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
    }

    // 构建响应内容
    let mut content: Vec<serde_json::Value> = Vec::new();

    if !text_content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": text_content
        }));
    }

    content.extend(tool_uses);

    // 估算输出 tokens
    let output_tokens = token::estimate_output_tokens(&content);

    // 使用从 contextUsageEvent 计算的 input_tokens，如果没有则使用估算值
    let final_input_tokens = context_input_tokens.unwrap_or(input_tokens);

    // 构建 Anthropic 响应
    let response_body = json!({
        "id": format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": final_input_tokens,
            "output_tokens": output_tokens
        }
    });

    // 记录成功的请求
    if let (Some(id), Some(pool)) = (&account_id, &pool) {
        let log = crate::pool::RequestLog {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: id.clone(),
            account_name,
            model: model.to_string(),
            input_tokens: final_input_tokens,
            output_tokens,
            success: true,
            error: None,
            timestamp: chrono::Utc::now(),
            duration_ms: start_time.elapsed().as_millis() as u64,
        };
        pool.add_request_log(log).await;
    }

    (StatusCode::OK, Json(response_body)).into_response()
}



/// POST /v1/messages/count_tokens
///
/// 计算消息的 token 数量
pub async fn count_tokens(JsonExtractor(payload): JsonExtractor<CountTokensRequest>) -> impl IntoResponse {
    tracing::info!(
        model = %payload.model,
        message_count = %payload.messages.len(),
        "Received POST /v1/messages/count_tokens request"
    );

    let total_tokens = token::count_all_tokens(payload.model, payload.system, payload.messages, payload.tools) as i32;

    Json(CountTokensResponse {
        input_tokens: total_tokens.max(1) as i32,
    })
}
