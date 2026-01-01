# kiro-rs

一个用 Rust 编写的 Anthropic Claude API 兼容代理服务，将 Anthropic API 请求转换为 Kiro API 请求。

## 功能特性

- **Anthropic API 兼容**: 完整支持 Anthropic Claude API 格式
- **流式响应**: 支持 SSE (Server-Sent Events) 流式输出
- **Token 自动刷新**: 自动管理和刷新 OAuth Token
- **Thinking 模式**: 支持 Claude 的 extended thinking 功能
- **工具调用**: 完整支持 function calling / tool use
- **多模型支持**: 支持 Sonnet、Opus、Haiku 系列模型
- **账号池模式**: 支持多账号轮询、负载均衡
- **Web 管理面板**: 可视化管理账号和监控状态
- **配额管理**: 实时查看账号剩余配额
- **请求记录**: 记录请求历史和统计信息
- **错误自动处理**: 账号限流自动冷却，暂停自动禁用

## 支持的 API 端点

### Anthropic API

| 端点 | 方法 | 描述 |
|------|------|------|
| `/v1/models` | GET | 获取可用模型列表 |
| `/v1/messages` | POST | 创建消息（对话） |
| `/v1/messages/count_tokens` | POST | 估算 Token 数量 |

### 管理 API（需要认证）

| 端点 | 方法 | 描述 |
|------|------|------|
| `/api/status` | GET | 获取服务状态 |
| `/api/accounts` | GET/POST | 获取/添加账号 |
| `/api/accounts/import` | POST | 导入 Kiro JSON 凭证 |
| `/api/accounts/{id}` | DELETE | 删除账号 |
| `/api/accounts/{id}/enable` | POST | 启用账号 |
| `/api/accounts/{id}/disable` | POST | 禁用账号 |
| `/api/accounts/{id}/usage` | GET | 获取账号配额 |
| `/api/accounts/{id}/usage/refresh` | POST | 刷新账号配额 |
| `/api/strategy` | GET/POST | 获取/设置负载均衡策略 |
| `/api/logs` | GET | 获取请求记录 |
| `/api/logs/stats` | GET | 获取请求统计 |
| `/api/usage/refresh` | POST | 刷新所有账号配额 |

## 快速开始

### 1. 编译项目

```bash
cargo build --release
```

### 2. 配置文件

创建 `config.json` 配置文件：

```json
{
   "host": "0.0.0.0",
   "port": 8080,
   "apiKey": "sk-your-custom-api-key",
   "region": "us-east-1"
}
```

### 3. 凭证文件

创建 `credentials.json` 凭证文件：

**Social 认证（最小配置）：**
```json
{
   "refreshToken": "XXXXXXXXXXXXXXXX",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "social"
}
```

**IdC / BuilderId 认证：**
```json
{
   "refreshToken": "XXXXXXXXXXXXXXXX",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "idc",
   "clientId": "xxxxxxxxx",
   "clientSecret": "xxxxxxxxx"
}
```

### 4. 启动服务

**单账号模式：**
```bash
./target/release/kiro-rs
```

**账号池模式（带 Web 管理面板）：**

*Linux / macOS:*
```bash
POOL_MODE=true ./target/release/kiro-rs
```

*Windows PowerShell:*
```powershell
$env:POOL_MODE="true"; ./target/release/kiro-rs
```

*Windows CMD:*
```cmd
set POOL_MODE=true
target\release\kiro-rs
```

## 运行模式

### 单账号模式（默认）

使用单个凭证文件运行，适合个人使用。

### 账号池模式

设置 `POOL_MODE=true` 启用，支持：
- 多账号管理
- 轮询 / 随机 / 最少使用 三种负载均衡策略
- 账号状态追踪（活跃/冷却/失效/禁用）
- Web 管理面板（访问 `http://服务地址/`）
- 账号持久化存储

> **注意**：开启账号池模式后，系统**不再**读取 `credentials.json` 文件。
> 账号数据存储在 `data/accounts.json` 中。您可以：
> 1. 启动服务后通过 Web 管理面板添加账号（推荐）
> 2. 手动创建 `data/accounts.json` 文件（参考根目录下的 `accounts.example.json`）

## 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `HOST` | 监听地址 | `0.0.0.0` |
| `PORT` | 监听端口 | `8080` |
| `API_KEY` | API 密钥 | - |
| `REGION` | AWS 区域 | `us-east-1` |
| `POOL_MODE` | 启用账号池模式 | `false` |
| `DATA_DIR` | 数据存储目录 | `./data` |
| `REFRESH_TOKEN` | OAuth 刷新令牌 | - |
| `AUTH_METHOD` | 认证方式 (social/idc) | - |
| `CLIENT_ID` | IdC 客户端 ID | - |
| `CLIENT_SECRET` | IdC 客户端密钥 | - |

## Docker 部署

```bash
docker build -t kiro-rs .
docker run -d \
  -p 8080:8080 \
  -e API_KEY=sk-your-key \
  -e POOL_MODE=true \
  -v /path/to/data:/app/data \
  kiro-rs
```

## Zeabur 部署

1. Fork 本仓库或直接导入
2. 添加持久化存储卷，挂载到 `/app/data`
3. 设置环境变量：
   ```
   POOL_MODE=true
   API_KEY=sk-your-api-key
   DATA_DIR=/app/data
   ```
4. 部署完成后访问服务地址即可看到管理面板

## Web 管理面板

账号池模式下，访问服务根路径即可打开管理面板（需要输入 API 密钥登录）：

### 功能概览

- 📊 **实时状态监控** - 运行时间、账号状态、请求统计、Token 用量
- 👥 **账号管理** - 添加、导入、启用/禁用、删除账号
- 📈 **配额查看** - 实时刷新账号剩余配额和使用进度
- 📝 **请求记录** - 查看最近 100 条请求历史（持久化保存最近 1000 条）
- 🔄 **负载均衡** - 切换轮询/随机/最少使用策略
- 🔐 **安全认证** - 使用 API 密钥保护管理面板

### 配额管理

点击账号列表中的 🔄 按钮可刷新单个账号配额，或点击工具栏的"刷新配额"批量刷新所有账号。

配额进度条颜色说明：
- 🟢 绿色：剩余 > 30%
- 🟡 黄色：剩余 10-30%
- 🔴 红色：剩余 < 10%

### 错误自动处理

- **429 限流错误**：账号自动进入 5 分钟冷却状态
- **403 暂停错误**：账号自动标记为失效状态
- 错误计数实时更新，方便排查问题账号

### 数据持久化

账号池模式下，以下数据会自动保存到 `DATA_DIR` 目录：
- `accounts.json` - 账号信息和状态
- `request_logs.json` - 请求记录（最多 1000 条）

### 导入 Kiro 凭证

支持直接粘贴 Kiro IDE 导出的完整 JSON：

```json
{
  "email": "xxx@example.com",
  "provider": "BuilderId",
  "refreshToken": "aorAAAAA...",
  "clientId": "...",
  "clientSecret": "...",
  "region": "us-east-1"
}
```

系统会自动识别认证方式并提取账号名称。

## 配置说明

### config.json

| 字段 | 类型 | 默认值 | 描述 |
|------|------|--------|------|
| `host` | string | `0.0.0.0` | 服务监听地址 |
| `port` | number | `8080` | 服务监听端口 |
| `apiKey` | string | - | 自定义 API Key |
| `region` | string | `us-east-1` | AWS 区域 |
| `kiroVersion` | string | `0.8.0` | Kiro 版本号 |
| `machineId` | string | 自动生成 | 自定义机器码 |
| `proxyUrl` | string | - | HTTP/SOCKS5 代理 |

### credentials.json

| 字段 | 类型 | 描述 |
|------|------|------|
| `accessToken` | string | OAuth 访问令牌（可选） |
| `refreshToken` | string | OAuth 刷新令牌 |
| `profileArn` | string | AWS Profile ARN（可选） |
| `expiresAt` | string | Token 过期时间 |
| `authMethod` | string | 认证方式（social/idc） |
| `clientId` | string | IdC 客户端 ID |
| `clientSecret` | string | IdC 客户端密钥 |

## 使用示例

```bash
curl http://127.0.0.1:8080/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-your-api-key" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "messages": [
      {"role": "user", "content": "Hello!"}
    ]
  }'
```

## 高级功能

### Thinking 模式

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 16000,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 10000
  },
  "messages": [...]
}
```

### 工具调用

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "tools": [
    {
      "name": "get_weather",
      "description": "获取天气",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": {"type": "string"}
        }
      }
    }
  ],
  "messages": [...]
}
```

### 流式响应

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "stream": true,
  "messages": [...]
}
```

## 技术栈

- **Web 框架**: Axum 0.8
- **异步运行时**: Tokio
- **HTTP 客户端**: Reqwest (rustls)
- **序列化**: Serde
- **日志**: tracing

## License

MIT

## 致谢

- [kiro2api](https://github.com/caidaoli/kiro2api)
- [proxycast](https://github.com/aiclientproxy/proxycast)
- [原项目](https://github.com/hank9999/kiro.rs)
