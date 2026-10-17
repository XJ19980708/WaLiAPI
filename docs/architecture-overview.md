# WaLiAPI 项目架构解析

> 解析对象：**v0.3.1**（`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json` 三处均为 `0.3.1`）
> 代码规模实测：Rust 约 **6.6 万行**（`src-tauri/src/` 9 个模块目录 + 36 个迁移），前端 `src/` 9 个页面 + Web 面板 `web/`，测试 **931 个** `#[test]` / `#[tokio::test]`
> 每条结论都附 `文件:行号` 证据，可自行复核。文末第 24 节列出**文档与实际代码不一致**的位置。

---

## 1. 定位与设计取舍

WaLiAPI 是一个**本地 / 内网优先**的 LLM API 网关：下游用统一协议接入，出口按渠道调度转换后转发到上游各种模型供应商；同时内置知识库 RAG、Wiki 知识引擎与 MCP Server，让 AI Agent 能直接检索私有知识。

理解这个项目的架构，抓住四个取舍就够了：

| 取舍 | 具体表现 |
|:---|:---|
| **单进程、单机** | SQLite 落盘 + 内存态会话/限速表，没有外部依赖（无 Redis、无 Postgres、无消息队列）。优点是零依赖可跑，代价是**不支持多实例写同一数据目录** |
| **一套代码两种产物** | 桌面端与 headless 服务端共用全部业务代码，差异只在"桌面专属交互"与静态资源内嵌方式（见第 2 节） |
| **前后端同构复用** | Web 管理面板**直接复用桌面端的前端源码树**，只在构建期用别名把 Tauri API 换成 HTTP 实现（见第 22 节） |
| **安全靠边界而非加密** | 上游密钥、网关 Key **明文存 SQLite**，文件系统权限就是安全边界；项目不做静态加密（README「安全边界与威胁模型」有明确声明） |

---

## 2. 一套代码，两种产物

`src-tauri/Cargo.toml:99-104` 的 feature 划分决定了产物形态：

| 产物 | 入口 | feature | 说明 |
|:---|:---|:---|:---|
| 桌面端 `waliapi` | `src-tauri/src/main.rs` | 默认 `desktop-ui` | Tauri 2 窗口 + 系统托盘；`default-run = "waliapi"` |
| Headless `waliapi-web` | `src-tauri/src/bin/waliapi-web.rs` | `--no-default-features --features embed-web` | 无窗口纯 HTTP 服务，内嵌 Web 管理面板 |

三个关键机制让同一套业务代码能服务两种形态：

1. **`desktop-ui` 门控所有桌面专属代码**。托盘、文件对话框、OAuth 唤起浏览器等一律 `#[cfg(feature = "desktop-ui")]` 包裹（`lib.rs:149-178`）。headless 构建必须 `--no-default-features`，否则 GTK/WebKit/DBus 会被链进二进制。

2. **release profile 为"剔除桌面代码"服务**（`Cargo.toml:106-113`）：`lto = "fat"` + `codegen-units = 1`，目的是让链接器彻底丢弃 headless 二进制中未引用的桌面端符号。同一处注释还警告**不要加 `panic = "abort"`**——代码里知识库解析等护栏依赖 `catch_unwind`，加上会让保护失效。

3. **`MockRuntime` 让 Tauri 命令在无窗口环境下可分发**。headless 通过 `tauri::test::mock_app()` 造一个假的 `AppHandle`，只是为了拿到 `State<'static, Arc<AppState>>` 供 `/admin/api/invoke` 分发使用（`server/router.rs:59-66`）：

```rust
fn mock_state_handle(state: &Arc<AppState>) -> tauri::State<'static, Arc<AppState>> {
    let mock = Box::leak(Box::new(tauri::test::mock_app()));
    mock.manage(state.clone());
    ...
}
```

> 「业务逻辑写成 Tauri Command，再用 MockRuntime 在服务端调用」是这个项目最特殊的一处结构决策。它换来的是**前后端交互代码零重复**，代价是 headless 二进制依然链接了 tauri 库（因此运行时镜像仍需保留 webkit/gtk/dbus 动态库，见 `Dockerfile:77-89` 的注释，以及第 3 节的风险提示）。

---

## 3. 技术栈

| 层 | 技术选型 |
|:---|:---|
| 前端（共用） | React 19 + TypeScript ~5.8 + Vite 7 + Tailwind CSS 4 + React Router 7 + lucide-react |
| 前端（Web 面板额外） | `web/package.json` 额外引入 `zustand` ^5 与 `@tanstack/react-query` ^5 |
| 后端 | Rust edition 2021 + Tauri 2 + Axum 0.8 + tower-http 0.6 + tokio |
| HTTP 客户端 | reqwest 0.12（`json` + `stream`），流式与非流式**两个不同的 client**（第 11 节） |
| 数据库 | sqlx 0.8（SQLite，runtime-tokio）+ 36 个 SQL 迁移 |
| 内容解析 | tree-sitter 0.25（8 种语言）+ `pdf-extract` + `pdfium-render`（OCR 渲染）+ `image` |
| 向量检索 | 自实现 HNSW（`bincode` 序列化到磁盘）+ SQLite FTS5 关键词路 |
| Token 计量 | `tiktoken-rs` 0.6（`cl100k_base`），仅在上游未返回 usage 时兜底 |
| 认证 | `argon2` 0.5 做口令哈希、`oauth2` 5 做 Codex/Kimi 登录 |
| 静态资源 | `rust-embed` 8（仅 `embed-web` feature） |
| 日志 | `tracing` + `tracing-subscriber`（`RUST_LOG` 可调）+ `tracing-appender`（按天滚动，保留 7 份） |

值得注意的几处：**没有** ORM（直接写 SQL + 手写 `models.rs`）、**没有** Redis（内存态用 `dashmap` 与 `tokio::sync::broadcast`）、**没有** JWT（管理会话是进程内 `SessionStore`）。

---

## 4. 仓库结构（以实际目录为准）

```
WaLiAPI/
├── src/                        # 前端主源码（桌面端直接用；Web 面板构建期复用）
│   ├── pages/                  # 9 个页面：Dashboard / Channels / AuthChannels / ApiKeys / Logs /
│   │                           #   KnowledgeBase / Usage / Settings / AppConfig
│   ├── components/             # ChannelForm、MappingSection、auth/、channel-form/、layout/ 等
│   ├── hooks/  lib/  types/
│   └── lib/runtime.ts          # ★ 统一命令传输层（桌面 IPC ⇄ 浏览器 HTTP），见第 22 节
├── web/                        # Web 管理面板子包
│   ├── vite.config.ts          # ★ 用 @app 别名复用 ../src，用别名把 @tauri-apps/* 换成 shim
│   └── src/
│       ├── lib/tauri-shim.ts   # invoke → POST /admin/api/invoke
│       ├── lib/auth.ts         # 面板 token 存取
│       └── pages/{LoginPage,ChangePasswordPage}.tsx   # 桌面端没有的两个页面
├── src-tauri/
│   ├── src/
│   │   ├── lib.rs              # 桌面入口：AppState、托盘、后台任务、命令注册表
│   │   ├── bin/waliapi-web.rs  # headless 入口：CLI 解析 + repair-stream-logs 子命令
│   │   ├── web_server.rs       # headless 启动流程：数据目录解析、后台任务、ctrl-c 退出
│   │   ├── server/             # Axum HTTP 层（8445 行）
│   │   │   ├── handlers.rs     # ★ 5435 行，数据面全部端点实现
│   │   │   ├── router.rs       # 路由装配 + 按鉴权域分组（CORS 作用域在这里定）
│   │   │   ├── admin.rs        # 服务端点 token 守卫（KB/Wiki 与 MCP）
│   │   │   ├── admin_auth.rs   # 管理员账号、口令哈希、会话、失败限速
│   │   │   ├── admin_routes.rs # /admin/api/* 路由 + CSRF 中间件
│   │   │   ├── event_bridge.rs # EventSink：桌面 emit 与 SSE 广播的统一出口
│   │   │   ├── request_id.rs   # X-Request-Id 关联键中间件
│   │   │   └── static_assets.rs# SPA fallback（rust-embed）
│   │   ├── core/               # ★ 转发核心调度（7356 行）
│   │   │   ├── route_plan.rs   # 3061 行：RoutePlan 生成、候选解析、优先级权重排序
│   │   │   ├── attempt.rs      # 1563 行：FailureClass + AttemptFlow 重试状态机
│   │   │   ├── plan_executor.rs# 执行 plan
│   │   │   ├── dispatcher.rs   # legacy 渠道选择
│   │   │   ├── proxy.rs        # legacy 非流转发路径
│   │   │   ├── stream_supervisor.rs # 流提交前/后的状态机 + 超时种类
│   │   │   ├── channel_identity.rs  # 渠道身份指纹（identity_revision）
│   │   │   ├── protocol_boundary.rs # route_plan 与 codec 之间的防腐层
│   │   │   ├── feature_flags.rs     # 路由能力开关
│   │   │   └── weighted_key.rs      # 多 Key 加权轮转
│   │   ├── endpoint_executor/  # ★ 端点执行器（9044 行）：请求驱动、SSE、用量估算
│   │   ├── protocol/           # ★ 协议转换（17201 行，74 文件）：codec/ + sse_bridge.rs
│   │   ├── adaptor/            # 上游适配器（1470 行）：openai / claude / deepseek / gemini / custom
│   │   ├── auth_provider/      # Auth 账号（8908 行）：Codex / Kimi OAuth、刷新、模型发现
│   │   ├── security/           # 安全审计（3344 行）：scanner / rules / redact
│   │   ├── services/           # 服务层（16970 行）：mod.rs + knowledge/ + wiki/ + mcp/
│   │   ├── commands/           # Tauri Commands（8459 行），前端所有交互的入口
│   │   ├── db/                 # Database 初始化、models.rs、repository.rs（3098 行）
│   │   ├── health_probe.rs     # 渠道主动探测循环
│   │   ├── semantic_cache.rs   # 语义缓存
│   │   ├── audit_log.rs        # 审计日志维护循环
│   │   ├── otlp_exporter.rs    # OTLP 增量导出（默认关）
│   │   ├── prompt_templates.rs # Prompt 模板种子与版本
│   │   └── settings_store.rs   # 设置存储抽象（tauri-store / JSON 文件）
│   ├── migrations/             # 001–036 共 36 个 SQL 迁移
│   └── tests/                  # 5 个集成测试
├── deploy/                     # caddy/Caddyfile.example、systemd/{unit,env.example}
├── scripts/                    # fetch-pdfium.sh、Gitcode release 同步脚本
├── docs/                       # 设计文档（本文件与 deployment-guide.md 也在其中）
└── .github/workflows/          # 6 个发布工作流
```

---

## 5. 启动流程与全局状态

### 5.1 两个入口的差异

| 步骤 | 桌面（`lib.rs:79-434`） | Headless（`web_server.rs:75-160`） |
|:---|:---|:---|
| 数据目录 | Tauri `app_data_dir()`（`lib.rs:201-204`） | `resolve_data_dir()`：显式参数 > `WALIAPI_DATA_DIR` > `XDG_DATA_HOME/<APP_IDENTIFIER>` > 平台默认（`web_server.rs:23-44`） |
| 日志 | 数据目录 `logs/`，不可写则回退 exe 同级（`lib.rs:89-100`） | 数据目录 `logs/`，回退 exe 同级（`bin/waliapi-web.rs:171-181`） |
| 初始化顺序 | `Database::new` → `ensure_initial_admin` → `AuthService` → `AppState` → `manage` | 同左，但 `Database::new_with_path` |
| 托盘/窗口 | 有（`lib.rs:149-197`） | 无 |
| 退出 | 事件循环常驻 | `tokio::signal::ctrl_c()` 后返回（`web_server.rs:157`） |
| 服务启动 | `server::start_server(state, Some(app))` | `server::start_server(state, None)` |

**两个容易踩的点**：

- `XDG_DATA_HOME` 分支会**追加应用标识符**（`web_server.rs:40-42` → `/data/waliapi.xiaofuge.cn`），而 Docker 里设的是 `WALIAPI_DATA_DIR=/data`（不追加）→ 数据直接落在 `/data/waliapi.db`。改这两个变量时注意语义不同。
- 桌面端 `AppState.data_dir` 用的是 Tauri 的 `app_data_dir()`，headless 用自己解析的路径——同一套代码在两种形态下数据目录来源不同，写迁移/落盘逻辑时别假设两者一致。

### 5.2 AppState：唯一的全局状态容器

`lib.rs:55-76` 定义，`AppState` 挂 12 个字段：

```rust
pub struct AppState {
    pub db: Arc<db::Database>,
    pub auth_service: Arc<auth_provider::service::AuthService>,
    pub login_sessions: Arc<commands::auth::LoginSessions>,
    pub server_port: Arc<RwLock<u16>>,
    pub server_running: Arc<AtomicBool>,
    pub server_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    pub test_receipts: Arc<TestReceiptStore>,   // 渠道测试回执，进程重启即失效
    pub admin_sessions: Arc<SessionStore>,      // 面板会话（内存，重启失效 → 需重新登录）
    pub login_throttle: Arc<LoginThrottle>,     // 登录失败限速（内存）
    pub events: EventSink,                      // 统一事件出口
    pub settings: SettingsStore,                // tauri-store / JSON 文件
    pub data_dir: PathBuf,
}
```

注意**会话与限速都是内存态**：容器重启后面板需要重新登录、失败计数清零。这是有意的——省掉了会话持久化与过期清理的复杂度。

### 5.3 启动时拉起 5 个后台循环

`web_server.rs:121-146`（桌面端 `lib.rs:247-272` 同样）：

| 循环 | 作用 | 默认行为 |
|:---|:---|:---|
| `audit_log::run_maintenance_loop` | 审计日志维护 | 常开 |
| `otlp_exporter::run_export_loop` | OTLP 增量导出 request_log（按 seq 游标） | 默认关 |
| `health_probe::run_probe_loop` | 渠道主动健康探测 | **默认开，300s 一轮** |
| `semantic_cache::run_maintenance_loop` | 语义缓存过期清理 | 未启用缓存时空操作 |
| `auth_provider::maintenance::run_maintenance_loop` | Auth 账号 token 巡检刷新 | 常开 |

启动时还会做一次 `prompt_templates::seed_if_empty`：空表时写入源码字面量 v1，保证升级后行为逐字节不变；写失败只告警不阻断（运行时回退编译期默认）。

### 5.4 监听地址解析优先级

`server/mod.rs:90-119`：**环境变量 > SettingsStore > 默认**（host 默认 `127.0.0.1`，port 默认 `8777`）。CLI 参数 `--host/--port` 会被 `web_server.rs:77-82` 写成环境变量再走同一条链。

另外 `start_server` 在绑定前会做**鉴权配置体检**（`server/mod.rs:24-43`），三类告警：token 过短、两个 token 相同、非回环绑定却缺 token（后者会列出"哪些端点已关闭、哪些面仍暴露"）。这是排查部署问题时的第一手线索。

---

## 6. HTTP 面：路由与三个凭证域

### 6.1 路由装配（`server/router.rs:68-149`）

```
Router
├── 数据面（API Key 鉴权 + 宽松 CORS）
│     POST /v1/chat/completions        POST /v1/completions(legacy)
│     POST /v1/responses               GET  /v1/responses/{id}/events  ← 断线续传回放
│     POST /v1/embeddings              GET  /v1/models
│     POST /v1/images/generations
│     POST /v1/audio/transcriptions    POST /v1/audio/speech
│     POST /v1/messages                POST /v1/messages/count_tokens  ← 默认 32MB body 上限
│     GET  /health
│     └── 最外层挂 request_id 中间件（X-Request-Id 标准化）
├── 服务路由（分组鉴权，无 CORS）
│     /api/kb/*   /api/wiki/*   ← require_admin（WALIAPI_ADMIN_TOKEN）
│     /mcp*                    ← require_mcp（WALIAPI_MCP_TOKEN）
├── /admin/api/*               ← 面板会话鉴权 + CSRF（无 CORS）
└── SPA fallback（静态资源，必须放最后）
```

### 6.2 三个凭证域（互不通用）

| 面 | 凭证 | 实现位置 |
|:---|:---|:---|
| 数据面 `/v1/*` | `sk-waliapi-*` 网关密钥（`Authorization: Bearer` 或 `x-api-key`） | `server/handlers.rs` 内校验 |
| 管理面 `/admin/api/*` | 用户名/密码登录会话（Cookie `waliapi_admin_token` 或 Bearer token） | `server/admin_routes.rs` |
| KB/Wiki REST `/api/kb`、`/api/wiki` | `WALIAPI_ADMIN_TOKEN` | `server/admin.rs:154-161` |
| MCP `/mcp*` | `WALIAPI_MCP_TOKEN` | `server/admin.rs:165-172` |

三处安全设计值得注意：

- **fail-closed**：token 未配置时端点整体关闭、一律 401，而不是无鉴权放行（`server/admin.rs:180-183`，并有测试 `service_endpoints_fail_closed_without_configured_tokens` 覆盖）。
- **恒定时间比较**：`token_matches` 手工实现了不随匹配位置提前返回的逐字节比较，防按字节探测 token 前缀的时序攻击（`server/admin.rs:131-141`）。
- **CORS 作用域被显式约束**：宽松 CORS 只挂在数据面（跨域调网关是设计用法），管理面/服务路由不附着 CORS 头，浏览器跨域读不到响应；管理面另有 CSRF 中间件（`server/admin_routes.rs:67-86`，变更类方法必须带 `X-Requested-With: XMLHttpRequest`）。测试 `cors_only_covers_data_plane_not_service_routes` 固化了这个边界。

---

## 7. 数据面请求生命周期

以 `POST /v1/chat/completions` 为例，完整链路如下（`server/handlers.rs:478` 起）：

```
handle_chat_completions                       server/handlers.rs:478
 ├─ JSON 解析 / API Key 鉴权 / 配额检查
 ├─ audit_original(...)                       server/handlers.rs:531   ← 安全门，扫原始协议全树（fail-closed）
 ├─ semantic_cache::lookup(...)               server/handlers.rs:574   ← 命中即短路返回
 ├─ maybe_route_plan(...)                     server/handlers.rs:631   ← 读 feature flags，决定走主轨还是 legacy
 │
 ├─【主轨 · new_routeplan 开启（生产默认）】
 │   route_plan::authorize_and_plan_with_accounts   core/route_plan.rs:664
 │        └─ 产出 RoutePlan（groups[]，每组含 candidates + max_attempts）
 │   ├─ 流式：driver::route_stream_plan_with_auth_service
 │   └─ 非流式：driver::route_plan_response_with_auth_service
 │         └─ core/plan_executor.rs: execute_plan        endpoint_executor/driver.rs:338
 │               └─ 每个 attempt：dispatch_channel_with_key_failover → dispatch_executor / dispatch_stream_executor
 │
 └─【legacy 轨 · flag 关闭或主轨返回 Ok(None)】
     流式：handle_stream                      server/handlers.rs:856
     非流式：proxy::handle_request            server/handlers.rs:662
```

**两轨并存**是重构期的过渡设计：`feature_flags.rs` 的 `new_routeplan` 在主轨读值处被**恒置 true**（`feature_flags.rs:59`），所以生产环境实际总是走主轨，legacy 路径保留是为了回退与测试对照。

`audit_original` 在路由之前、缓存之前执行，顺序很关键：**安全扫描不允许被缓存命中绕过**，且审计命中会使请求失去可缓存资格（`semantic_cache.rs:54` 的 `cacheable` 会排除审计命中）。

---

## 8. 渠道调度：候选解析、排序与 Key 轮转

### 8.1 候选来自两个源

调度候选 = **渠道（channels）** 与 **Auth 账号（auth_accounts）** 合并而成（`route_plan.rs:664` `resolve_route_candidates`）。legacy 轨只有渠道（`dispatcher.rs:22` `select_channels`）。账号参与路由是 v0.3.x 的新能力，用于 Codex/Kimi 这类"登录态即凭证"的上游。

### 8.2 优先级 + 权重：无放回加权抽样

`order_by_priority_weight`（`route_plan.rs:415`）：

- 先按 `priority` **降序**分档——高优先档整体优先，跨档不混合；
- 同档内按 `weight` 做**无放回加权抽样**：累减权重、`pick < 0` 命中，保证同档内每个候选在本次排序中**只出现一次**（不会因权重高而重复霸榜）。

### 8.3 多 API Key 轮转

`weighted_key.rs:22-26`：

```rust
let mut pick = rand::rng().random_range(0..total);   // 半开区间 [0, total)
...
if pick < 0 { ... }                                  // 累减后命中
```

注释里记录了这个 `< 0`（而非 `<= 0`）修的是 issue #34：旧写法的边界让**第二个 Key 恒轮空**。这类"边界差异导致某个 Key 永远用不上"的问题很难从监控上看出来，值得留意。

Key 级重试发生在渠道内部：`dispatch_channel_with_key_failover`——同一渠道的多个 Key 之间切换，不换渠道。

### 8.4 执行

`RoutePlan`（`route_plan.rs:307`）由 `RouteGroup`（`:264`）组成，每组带 `candidates` 与 `max_attempts`；`plan_executor::execute_plan` 按组推进，逐 attempt 执行。

---

## 9. 故障分类与重试降级

### 9.1 六类失败（`core/attempt.rs:31-46`）

| FailureClass | 触发条件 | 是否可跨组降级 |
|:---|:---|:---|
| `CallerTerminal` | 本地 schema/编解码不支持、上游 400/422 | 否，立即结束 |
| `ChannelAuthTerminal` | 上游 401/403 | 否（组内可换候选） |
| `EndpointUnsupported` | 405/501；或 404 **经响应体证明**是路径不存在 | **是** |
| `Retryable` | 连接失败、超时、408/409/429/5xx/529 | **是** |
| `UpstreamProtocolError` | commit 前上游响应无法解码 | 否（组内可换候选） |
| `CommittedStreamError` | 下游已 commit | 否，且不重试，只能向下游报协议内错误 |

`is_degradable()` 只认 `EndpointUnsupported` 与 `Retryable`（`attempt.rs:61-66`）。`UpstreamProtocolError` 被**刻意排除**在跨组之外，`attempt.rs:52-60` 的注释解释了裁决理由：不允许用"悄悄换到另一种协议组"来掩盖上游语义/权限错误——否则排障时会被误导。

### 9.2 重试状态机（`AttemptFlow::next_step`，`attempt.rs:511`）

- 组内候选耗尽或该组预算用完 → 若 `last_crossable` 才切下一组（`attempt.rs:522`）；
- 总尝试次数受 `max_attempts_total` 约束，避免"多组 × 多候选"乘积爆炸；
- **404 的判据不是状态码而是响应体**：只有响应体能证明"路径不存在"才算 `EndpointUnsupported`，否则降级为 `Retryable`——因为很多上游会用 404 表达别的东西。

### 9.3 流式提交边界（`core/stream_supervisor.rs`）

纯状态机，核心是 `swapable()`（`:49`）：**提交前**（尚未向下游发出任何字节）可以自由换上游；**提交后**禁止重试。同时定义 `StreamTimeoutKind`（`:72`）区分三种超时：连接、首帧、空闲——三者语义不同，混成一个超时会误判（比如首帧慢但流健康，不该按空闲超时掐）。

---

## 10. 协议转换层

### 10.1 9 条方向矩阵

`codec/registry.rs:180` 的 `CodecRegistry::direction` 用 `match (downstream, upstream)` 落出转换策略：

- **3 条 identity**：Chat→Chat、Messages→Messages、Responses→Responses（同协议直通）
- **6 条跨协议**：Chat↔Messages、Chat↔Responses、Responses↔Chat、Messages↔Responses

其中两条较重的策略单独成文件导出：`MESSAGES_TO_RESPONSES_V2` / `RESPONSES_TO_MESSAGES_V2`（`codec/directions/mod.rs`），其余内联在 registry。

`CodecDirection` trait（`codec/direction.rs:12`）只定义三件事：`encode_request` + 非流式 response decoder + 流式 response decoder。接口刻意收窄——转换层不碰路由、不碰重试。

### 10.2 防腐层 `protocol_boundary.rs`

这是 route_plan 与 codec 之间**唯一**的翻译点（`protocol_boundary.rs:43` `upstream_protocol`）。它解决的是"OpenAI 不是单一协议"的问题：

- **下游端点** → 映射到三种协议之一；
- **上游**必须 **(协议, 端点) 成对匹配**——同为 OpenAI，`/chat/completions` 与 `/responses` 是不同端点能力；
- 返回 `None` 表示走原生旁路（`count_tokens`、`embeddings` 这类不需要转换的端点）。

没有这层，`UpstreamProtocol` 会被压缩成"OpenAI / Anthropic"二选一，Responses 与 Chat 的差异就被抹掉了。

---

## 11. 流式 SSE

### 11.1 字节级重组（为什么不能用 `str::from_utf8` 门禁）

`protocol/sse_bridge.rs:58` 的 `push` 刻意签名为 `&[u8]` 而非 `&str`。原因写在注释里：TCP/HTTP 的 chunk 边界**可能切断一个 UTF-8 码点**，而中文一个字 3 字节，被切断的概率不低。正确做法是字节级缓冲、只对**完整记录**解码、跨调用保留残余字节。若用 `str::from_utf8` 做门禁，遇到半个汉字就会整块丢弃——表现为**偶发的中文乱码或丢字**，且极难复现。

`UpstreamSseBridge` 的职责是把任意上游 SSE 归一成 OpenAI 的 `data: {json}\n\n` 形态：OpenAI 上游用 `ResponsesSseAssembler` 重组，Anthropic 上游用 `MessagesSseState` 转换，并归一化 `[DONE]`（去掉 serde 给字符串加的引号）。

### 11.2 两个 HTTP 客户端

`adaptor/mod.rs`：

| 客户端 | 位置 | 超时策略 |
|:---|:---|:---|
| `streaming_client()`（单例） | `:69` | **只设 connect 超时**，body 读取不设总超时——长文本生成不能有总超时 |
| `blocking_client(timeout_secs)` | `:35` | 按渠道 timeout 分桶缓存，设总超时（渠道级配置，默认 60s） |

改超时逻辑时必须区分两者：给流式加总超时会让长回答被腰斩，给非流式去掉总超时会让慢上游拖垮请求。

流式另有 `StreamTimeouts`（`endpoint_executor/driver.rs:1515`）：首帧 60s / 空闲 120s，可配置。

### 11.3 断线续传（Responses 专有）

`GET /v1/responses/{id}/events?offset=N` 支持从指定序号回放已生成帧：逐帧落库（见第 21 节 `stream_segments`），回放时若原流没有终止帧会**合成一个 `incomplete` 完成帧收尾**，避免客户端永久挂起。超出保留期返回 `410`，未知 id 返回 `404`（测试 `responses_replay_replays_frames_and_synthesizes_incomplete_tail` / `responses_replay_unknown_id_and_expired_window` 固化了行为）。

---

## 12. Token 计量

两级策略：

1. **优先取上游返回值**：`extract_usage`（`endpoint_executor/mod.rs:1200`）兼容四种形态——Anthropic 的 `input_tokens/output_tokens`、Responses 的 `input_tokens`、Chat 的 `prompt_tokens`、以及 `cached_tokens`。
2. **上游未返回时本地估算**：`estimate_usage`（`endpoint_executor/estimate_usage.rs:26`）用 tiktoken-rs 的 `cl100k_base`：
   - prompt = 各消息文本 token 之和 + **每条消息 4 token 的 framing 开销**；
   - completion = 响应正文 token；
   - BPE 编码器做成进程内单例（`:148` `CL100K_SINGLETON`），避免每请求重建（tiktoken 构造开销远大于编码本身）。

触发估算的条件是三者同时为 0 且状态码 2xx（`driver.rs:458`）。写入 `request_logs` 的 `prompt_tokens/completion_tokens` 就来自这条兜底路径。

一个待补齐的地方：migration `028_request_log_usage_source.sql` 新增了 `usage_source` 列用于标注 token 来源，但**实时转发路径不写这一列**——搜索全仓，只有离线修复命令 `commands/log_repair.rs` 会写它（`log_repair.rs:440`、`:476`，值为 `repaired`）。所以正常请求的 `usage_source` 是 `NULL`，无法从库里区分"上游实测"与"本地估算"。

---

## 13. 渠道健康：两条独立的健康信号

项目里"渠道健康"是**两套互不写对方表**的机制，混淆它们会得出错误结论：

| | 主动探测（probe health） | 模式健康（mode health） |
|:---|:---|:---|
| 触发 | 定时器，默认 300s 一轮 | 真实业务请求的结果 |
| 粒度 | **渠道级** | **渠道 × 端点 × 流式** 三元组 |
| 探测动作 | `GET {base_url}/models`，10s 超时 | 无（被动记录） |
| 存储 | `channels.last_probe_ok` / `probe_latency_ms`（migration `033_channel_probe.sql:5`） | 独立表 `channel_mode_health`（migration `020`） |
| 失败后果 | 该渠道在候选排序中**沉底但不剔除** | 置冷却期（默认 5 分钟）后跳过该三元组 |
| 实现 | `health_probe.rs:33` | `endpoint_executor/driver.rs:210` `record_channel_mode_outcome` |

**模式健康的写入条件非常窄**：只有 `FailureClass == UpstreamProtocolError` 且错误信息以特定前缀开头（"undecodable body" / "could not be decoded" / "stream ended before a valid first SSE record" / "first frame could not be converted"）才判定为"渠道能力问题"（`driver.rs:194` `affects_mode_health`）。这由 `driver.rs:195-207` 的四条前缀白名单实现。

设计意图很明确：**CallerTerminal（用户请求有问题）、ChannelAuthTerminal（密钥问题）、限流都不该标记渠道"能力不行"**——否则一个客户端发错格式就能把健康渠道打进冷却。

主动探测的"沉底不剔除"也是有意为之：探测失败可能只是探测端点不可用，直接剔除渠道会让可用容量凭空消失；沉底则保留了兜底可能性。

---

## 14. 语义缓存

### 14.1 先判定"能不能缓存"（`semantic_cache.rs:54` `cacheable`）

排除四类：审计命中、带 `tools`/`tool_choice`、`temperature > 0.3`、消息无法规范化。前两条是正确性（工具调用与安全命中的响应不应复用），后两条是有效性（高温采样本来就不该命中）。

### 14.2 两层命中

| 层 | 键 | 成本 |
|:---|:---|:---|
| **exact** | `SHA-256(规范化消息 + model + 温度档 + max_tokens 档)` | O(1) 查表（`semantic_cache.rs:71`） |
| **semantic** | 取最后一条 user 文本的向量，对**同 model 未过期**条目暴力算余弦，取最高分 | 线性扫描；阈值 `cache.semantic_threshold_percent` 默认 **95%**（`:165`、`:193`） |

注意 exact 键里放的是**温度档 / max_tokens 档**而非精确值——这是刻意的归一化，让 `0.00` 与 `0.05` 能命中同一条。也正因如此，`temperature > 0.3` 必须先被排除，否则会把不同采样强度的结果混为一谈。

### 14.3 命中的位置与记账

命中发生在**安全门之后、渠道路由之前**（`handlers.rs:574`）：非流式回 `replay_body`，流式回 `replay_sse_frames`，并插入 `X-Cache: hit` 响应头（`handlers.rs:621`）。

**命中照常记账**——会写一条 `upstream_type = 'cache'` 的请求日志（测试 `semantic_cache_hit_short_circuits_upstream_with_hit_header` 断言了这点）。这样既省了上游费用，又不让用量统计出现"凭空少了一堆请求"的空洞。

写入在 2xx 完整落账之后 best-effort 执行（`handlers.rs:693`）。

---

## 15. 服务层：Service trait 与注册表

`services/mod.rs:16-31` 定义 `Service` trait，只有 6 项：`id / name / description / enabled / status / routes`。`ServiceRegistry::new()` 注册三个实现：

| 服务 | id | 挂载鉴权 |
|:---|:---|:---|
| KnowledgeService | `knowledge` | `require_admin`（WALIAPI_ADMIN_TOKEN） |
| WikiService | `wiki` | `require_admin`（WALIAPI_ADMIN_TOKEN） |
| McpService | `mcp` | `require_mcp`（WALIAPI_MCP_TOKEN） |

装配方式是按 id 过滤合并（`services/mod.rs:51-82` `merge_routes_for`），在 `server/router.rs:80-93` 完成分组。这个抽象的价值在于：**新增一类服务只需实现 trait 并注册，路由分组与鉴权自动落到对应凭证域**，不必改 `router.rs`。同时 `enabled/status` 让 `commands::services::get_service_statuses` 能统一汇报各服务状态给面板。

---

## 16. 知识库 RAG 全链路

### 16.1 摄入：解析 → 分块 → 向量化 → 索引

```
parse_file                        services/knowledge/parser.rs:21-73
  ├─ 按扩展名分派：PlainText / Markdown / Code / Structured
  ├─ PDF 走 pdf_extract；解析 panic 用 catch_unwind 兜底（这就是不能开 panic=abort 的原因）
  └─ 可选 OCR（见 16.3）
split                             splitter.rs:14-15
  ├─ 默认 512 token / 64 overlap（约 4 字符 ≈ 1 token 的估算）
  ├─ Markdown 先按标题分节；代码按 tree-sitter 符号分块（code_parser.rs:51-59）
  └─ 超长块再按大小切分
embedder::embed                   embedder.rs:8-31
  └─ 复用渠道的 OpenAI 兼容 /embeddings，带 model_mapping 与逐渠道 failover
     默认 text-embedding-3-small，向量维度自动探测（:76）
落 kb_chunks + 构建 HNSW          index.rs:90
```

**tree-sitter 支持 8 种语言**：TypeScript / TSX / JavaScript / JSX / Python / Rust / Go / Java。这让代码文件能按符号边界分块，而不是把函数拦腰截断。

### 16.2 检索：HNSW + FTS5 双路融合

- **向量路**：自实现 HNSW（余弦距离，`ef_construction=200` / `ef_search=50`，`retriever.rs:10-20`），索引用 `bincode` 序列化到 `hnsw_indexes/kb_{id}.hnsw`；
- **关键词路**：SQLite FTS5 虚表 `kb_chunks_fts`（migration `012_fts5_hybrid_search.sql:5`）；
- **融合**：默认 **RRF**（k=60），也支持线性加权（`retriever.rs:686-718`、`:730-784`）。

自实现 HNSW 而不是引第三方向量库，是这个项目"零外部依赖"取舍的直接体现：换来部署简单，代价是索引能力（更新、删除、压缩）需要自己维护。

### 16.3 OCR 子模块（扫描版 PDF）

默认关闭（`ocr.enabled`），且只在 PDF 上尝试。策略是**先零成本判断**：

1. 先逐页提取文字层；**单页字数 < 50 才判定为需要识别**——纯文字 PDF 因此完全不触发 OCR；
2. 需要识别时才用 pdfium（运行时加载动态库，全局 `tokio::Mutex` 串行）按 `dpi/72` 渲染成 JPEG；
3. 交给 `VlmOcrClient` 走渠道的 `/chat/completions` 视觉格式（兼容 Anthropic 的 image source），带跨渠道 failover + 单页重试 1 次；
4. 有页级缓存、页数上限、**失败过半熔断**；输出带 `<!-- page: N -->` 锚点的 Markdown。

pdfium 是动态库、不入版本库（`resources/pdfium/` 只有 README），由 `scripts/fetch-pdfium.sh` 在打包/构建时下载并校验（大小 >1MB + ELF 魔数 `7f454c46`）。

### 16.4 导入安全（`import_guard.rs`）

这是全项目安全防护最密集的一处，因为"导入"是唯一让用户输入影响服务端网络行为的入口：

| 入口 | 防护 |
|:---|:---|
| git 仓库 | 仅允许 https，**剥离 URL 中的 userinfo 与 query**；阻止 branch 选项注入；token 不拼进 URL，失败信息做消毒 |
| URL 导入 | scheme 白名单 + **DNS 全记录**内网/保留地址判定（含 `169.254` 云元数据地址）→ 防 SSRF；仅同主机重定向且逐跳复检；10MB 上限 |
| 本地目录 | `canonicalize` 后按白名单根校验，防符号链接逃逸 |

---

## 17. Wiki 知识引擎

### 17.1 摄入管道（`wiki/ingest.rs:130-201`）

```
来源文件落盘 → 按类型解析分节 → 调渠道 LLM 生成页面
   → 强制 YAML frontmatter（title / type / tags / source），正文用 [[wikilinks]]
   → 双写：磁盘 wiki/{project}/wiki/*.md  +  wiki_pages 表行
        (path / title / page_type / content_hash / wikilinks / frontmatter / tags)
   → 提取 wikilink → 重建图谱
```

**双写（磁盘 + 数据库）**是这里最值得注意的设计：Markdown 文件是"人可读可 git 的真相"，`wiki_pages` 行是"可查询可检索的索引"。`content_hash` 用于判断是否需要重建。选 Markdown + frontmatter 而不是纯数据库存储，是为了让知识资产保持可迁移——即使不用这个工具，文件也是标准的。

### 17.2 图谱

边落在 `wiki_graph_edges(source_page, target_page, edge_type='wikilink', weight)`。写入用**单事务分块批量 INSERT**（`wiki/repository.rs:169-184`），注释显示这里曾消除过 N+1 问题。图查询按 weight 排序返回节点 + 边，供面板可视化。

相关表还有 `wiki_sources`（来源）、`wiki_ingest_queue`（摄入队列）、`wiki_sessions`、`wiki_reviews`（评审）。

---

## 18. MCP Server

### 18.1 传输形态（`mcp/mod.rs:63-80`）

- **Streamable HTTP**：`POST /mcp` 直接回 JSON-RPC 响应；
- **GET + SSE 升级**：`/mcp` 同时支持 SSE 升级；
- `/mcp/` 兼容尾斜杠；`/mcp/sse` 保留旧式 SSE 端点；
- SSE 会话用**有界通道（容量 64）+ `try_send`**：慢客户端的事件直接丢弃而不是堆积内存；`Drop` 守卫保证断连即清理会话。

这是长生命周期连接的正确处理方式——无界队列 + 慢客户端等于内存泄漏。

### 18.2 29 个工具

| 类别 | 内容 |
|:---|:---|
| 读类 5 | `search` / `list` / `read` / `ask` / `stats` |
| KB 写类 | KB 增删改、文档上传/删除/列表、`build_index`、`import_source` |
| Wiki 类 | 项目 CRUD、页面读写删、`search` / `ask`、`tags` / `graph`、来源增删与 `ingest` |

JSON-RPC 分发在 `dispatch_jsonrpc_async`（`mcp/handlers.rs:546-593`），处理 `initialize`（protocolVersion `2024-11-05` + instructions）、`notifications/initialized`、`tools/list`、`tools/call`、`ping`。

注意 `initialize` 返回的 protocolVersion 是 `2024-11-05`，而路由层测试里客户端发的是 `2025-03-26`——两者都被接受，说明实现是宽松协商而非严格校验（`server/router.rs:287`）。

---

## 19. 安全审计

### 19.1 规则库（`security/rules.rs:55-281`）

内置 **25 条规则 / 8 类**：credential 5、file 3、infra 1、personal 2、unicode 4、network 4、tool 4、prompt 2。启动时 `INSERT OR IGNORE` 播种到 `security_builtin_rules`，可禁用/改级别；另有用户自定义规则表 `security_custom_rules`。

### 19.2 扫描时机与处置

扫描挂在**各协议入口**（`server/handlers.rs:552` 等），请求用 `scan_request`，响应用 `scan_response` / `scan_response_into`。

处置由 `decide_action` 按模式决策（`security/mod.rs:160-206`）：

| mode | 行为 |
|:---|:---|
| `off` / `audit` | 放行（只记录） |
| `warn` | ≥ Medium 告警 |
| `redact` / `confirm` / `block` | ≥ High 时分别脱敏 / 需确认 / 阻断 |

另有一条兜底：`block_on_critical` 打开时 Critical 级别**强制阻断**，不受 mode 影响。

### 19.3 脱敏的双信任边界（`security/redact.rs`）

这里有一个容易被忽略但很重要的区分：

- `redact_json`：**仅在 mode=redact 且开启 redact_secrets 时**才改写**转发体**；
- `redact_json_for_logging`：**无条件**用于**落库**。

即"转发出去的内容"与"记录下来的内容"是**不同的信任边界**：日志里必须总是脱敏（否则日志变成敏感信息堆积地），但转发体是否脱敏取决于用户策略（脱敏可能破坏上游请求的语义）。

`mask_string` 保留首尾 + `****`；Bearer 替换为 `[REDACTED]`；私钥整块替换；敏感字段名（`token` / `access_token` 等）按名单匹配。

### 19.4 边界声明（对自己能力的诚实标注）

README 明确写了这是"**尽力而为的数据泄露防护**"，不是完整内容安全方案。具体语义：

- 响应侧扫描覆盖非流式、流式与原生 Anthropic 路径；
- **扫描异常不阻断响应转发**（fail-open，避免扫描器故障导致全站不可用）；
- **但扫描预算超限按 fail-closed 拒绝请求**（避免"扫描不过来就放行"变成绕过手段）。

---

## 20. Auth 账号体系（auth_provider）

与"渠道"并列的第二类上游凭证来源，适合 Codex / Kimi 这类**登录态即凭证**的服务。

### 20.1 两条登录流程

| Provider | 流程 | 实现 |
|:---|:---|:---|
| Codex | 主路径 **PKCE + 回环回调** `http://localhost:{port}/auth/callback`；备路径 **RFC 8628 device code**；支持导入 CLI `auth.json` / CPA 格式；JWT 过期时先刷新 | `codex_login.rs:41-49`、`:281`、`:496-519` |
| Kimi | 纯 device code（`/api/oauth/device_authorization` → `/api/oauth/token`，refresh_token grant） | `kimi_login.rs:23-24`、`:102-111` |

`kimi_login.rs` 的 Debug 实现只打印字段**存在性**而不打印值，避免 token 通过调试日志泄露。

### 20.2 刷新与并发控制

`AuthService` 给**每个账号一把 mutex** 串行化刷新（`service.rs:290-363`），避免并发请求同时触发同一账号的刷新（refresh token 通常是一次性的，并发刷新会互相作废）。

- `needs_refresh` / `refresh_account_if_due` / `force_refresh_account` 三个入口；
- **401 仅内部重试一次**（防死循环）；
- `maintenance.rs` 定时巡检兜底。

### 20.3 存储与路由

- 凭证存 `auth_accounts.payload_json`（`provider` + `account_id` 唯一约束，migration `019`）；
- 模型快照存 `model_states_json`，配额存 `quota_json`；
- **与 channels 表无外键**，两者相互独立；`request_logs.upstream_type` 用 `channel` / `auth_account` 区分来源；
- 账号按 provider + `model_states` 参与路由候选（`auth_provider/mod.rs:147-158`、`spec.rs:55-77`）。

> 无外键是刻意的：账号与渠道是"可互相替代的上游凭证"，不是主从关系。

---

## 21. 数据层

### 21.1 初始化顺序（`db/mod.rs:119-193`）

```
建数据目录
 → 连接 waliapi.db（max_connections = 5，默认 journal_mode=delete，未启用 WAL）
 → fix_legacy_migration_checksums()
 → backup_before_migration()   ← 仅当已有迁移记录且版本落后时，VACUUM INTO 快照
      文件名 waliapi.db.pre-upgrade-<时间戳>，保留最近 3 份，失败不阻断
 → sqlx::migrate!  跑 36 个迁移
 → 播种安全规则
```

`journal_mode=delete`（回滚日志）是**运维上必须知道的一件事**：写事务提交需要 EXCLUSIVE 锁，因此与在线写入并行的维护操作（如 `repair-stream-logs`）会互相 `SQLITE_BUSY` 停顿。这也是那条维护命令反复强调"先停实例"的根因。

### 21.2 迁移按主题归类（36 个）

| 主题 | 迁移 |
|:---|:---|
| 渠道 | 001、014、015、020（mode health）、023、033（probe） |
| 请求日志 | 001、002、005、006、007、016（可观测列）、026、027、028（usage_source）、029、030（日志策略）、032（stream_segments） |
| 知识库 | 008–013、025、031（009 即 `kb.mcp_enabled`） |
| Wiki | 017、018 |
| 安全审计 | 003、004 |
| Auth 账号 | 019、021、036 |
| 面板与配置 | 022、024、034（prompt_templates）、035（semantic_cache） |

### 21.3 核心表

**网关侧**：`channels`、`channel_api_keys`（多 Key）、`channel_mode_health`、`api_keys`（`sk-waliapi-*`）。

**日志侧**：`request_logs`、`stream_segments`。

`request_logs` 的列可以分四组理解（`db/models.rs:356-406`、`migrations/016`）：

- 主信息：`seq`、`api_key_id/name`、`channel_id/name`、`model` / `upstream_model`、`mode`、`status_code`、`duration_ms`、`is_stream` / `is_retry`、`error_message`、`trace_id`、`upstream_type`
- Token：`prompt/completion/total_tokens`、`cached_tokens`、`usage_source`
- 可观测：`downstream_protocol` / `downstream_endpoint` / `route_group` / `upstream_protocol` / `upstream_endpoint` / `provider` / `codec_version` / `failure_class` / `identity_revision` / `client_cancelled` / `stream_committed`
- 风险：`risk_level` / `risk_score` / `risk_summary` / `security_action` / `sanitized` / `blocked_reason`

**流式大内容不落主表**，写溢出表 `stream_segments(log_id, seq, response_id, content, created_at)`（migration `032`）：`detail_level=detailed` 时随流式落账写入；`response_id` 是 Responses 断线续传的锚点（第 11.3 节），保留期与主表同步，`idx_stream_segments_response` 支撑回放查询。

**知识库侧**：`kb_knowledge_bases` / `kb_documents` / `kb_chunks`（+ FTS5 虚表 `kb_chunks_fts`）/ `kb_sources` / `kb_tasks` / `kb_index_meta` / `kb_conversations`。

**Wiki 侧**：`wiki_projects` / `wiki_pages` / `wiki_sources` / `wiki_ingest_queue` / `wiki_graph_edges` / `wiki_sessions` / `wiki_reviews`。

**其他**：`auth_accounts`、`request_security_findings`、`security_builtin_rules` / `security_custom_rules`、`admin_users`、`prompt_templates`、`semantic_cache`。

### 21.4 请求关联键

`server/request_id.rs` 中间件挂在数据面最外层：解析优先级 `X-Request-Id` > `Wali-Trace-Id`（旧头兼容）> 生成 UUIDv4；写回请求头、回显响应头。

闭环由测试固定：**请求头带的值 == 响应头回显值 == 落库 `request_logs.trace_id`**（`server/router.rs` 的 `request_id_echoed_header_equals_persisted_trace_id`，用本地 axum mock 当上游跑完整链路）。非法值（如超长）会被替换为生成值但仍回显，避免客户端拿到空关联键。

---

## 22. 前端架构

### 22.1 统一命令传输层（`src/lib/runtime.ts`）

前端所有后端交互走同一个 `invoke(command, args)`：

```ts
export async function invoke<T>(command: string, args = {}): Promise<T> {
  if (isTauriRuntime()) return tauriInvoke<T>(command, args);   // 桌面：Tauri IPC
  const token = getWebAdminToken();
  const response = await fetch("/admin/api/invoke", {          // 浏览器：同一个命令名 + 参数
    method: "POST", credentials: "same-origin",
    headers: { "content-type": "application/json",
               "x-requested-with": "XMLHttpRequest",            // ← CSRF 中间件要求
               ...(token ? { authorization: `Bearer ${token}` } : {}) },
    body: JSON.stringify({ cmd: command, args }),
  });
  ...
}
```

**命令名与参数对象在两种形态下完全一致**——后端 `/admin/api/invoke` 直接把 `cmd` 分发给同名的 Tauri Command。这就是"业务逻辑写成 Command"这一决策在前端的对应面：前端不需要为 Web 版写第二套 API 客户端。

注意 `x-requested-with` 头是必需的——它就是管理面 CSRF 中间件要求的那个头（第 6.2 节）。

### 22.2 Web 面板如何复用桌面前端（`web/vite.config.ts`）

```ts
resolve: { alias: [
  { find: "@app",              replacement: "../src" },              // 复用整个前端源码树
  { find: "@tauri-apps/api/core",        replacement: "tauri-shim.ts" },
  { find: "@tauri-apps/api/event",       replacement: "tauri-event-shim.ts" },
  ...  // opener / dialog / updater / process 也各有 shim
]}
```

两类替换让 `../src` 的页面在浏览器里能跑：

1. **`@app` 别名**：`web` 子包直接用桌面端的 `pages/`、`components/`、`hooks/`、`types/`；
2. **Tauri API → shim**：`web/src/lib/tauri-shim.ts` 把 `invoke` 实现为 `POST /admin/api/invoke`，并在 401 时清 token 跳登录页。

`web/src/pages/` 只多出两个桌面端不存在的页面：`LoginPage.tsx`、`ChangePasswordPage.tsx`。整个 Web 面板的"新增代码量"基本就等于这两个页面 + 7 个 shim —— 这是同构复用最直接的红利。

### 22.3 事件通道：桌面 emit ⇄ 浏览器 SSE

`server/event_bridge.rs` 的 `EventSink` 是统一出口：

```rust
pub fn emit(&self, event: &str, payload: Value) {
    if let Some(emit) = &self.desktop_emit { emit(event, payload.clone()); }  // 桌面：Webview 事件
    let _ = self.tx.send(AdminEvent { ... });                                // 两态共用：broadcast
}
```

- 桌面形态：`EventSink::desktop(...)` 同时投 Webview 与广播；
- headless 形态：`EventSink::headless(tx)` 只投广播。

广播由 `GET /admin/api/events`（SSE）消费（`admin_routes.rs:370` 的 `events_handler`）。前端 `runtime.ts:81-156` 的 `listen()` 在浏览器侧实现了 SSE 解析 + **断线指数退避重连（1s 起、上限 30s，连接成功即复位）**，保证服务重启或网络抖动后渠道测试进度这类事件不静默丢失。

广播通道容量 100（`event_bridge.rs:15`），**超出后最旧事件被丢弃**，落后的客户端会在 `recv` 时收到 `Lagged` 并跳过（`admin_routes.rs:381-384` 显式处理）——设计上选择"丢旧事件"而不是"阻塞生产者"。

### 22.4 前端状态管理

- 根 `src/`：函数组件 + hooks，**无**全局状态库（`src/package.json` 依赖里没有 zustand / react-query）；
- `web/` 子包：**额外引入了** `zustand` ^5 与 `@tanstack/react-query` ^5（`web/package.json`）。

所以"本项目不用状态库"这句话只对桌面端前端成立。Web 面板因为是登录态 + 长会话场景，选了这两个库。**改动前端时先确认自己改的是哪一侧**。

---

## 23. 扩展指引

| 想做什么 | 该动哪里 |
|:---|:---|
| 新增上游适配器 | `adaptor/`（参考 openai / claude / gemini 的现有实现），并在渠道预设注册表 `channel_presets.rs` 登记 |
| 新增协议转换方向 | `protocol/codec/registry.rs` 的 `(downstream, upstream)` 矩阵 + `codec/directions/`；同时确认 `protocol_boundary.rs` 能映射出对应的 (协议, 端点) 对 |
| 新增一类服务（如"报表"） | 实现 `services::Service` trait（6 个方法）并在 `ServiceRegistry::new()` 注册 + `router.rs` 里归入某个凭证域 |
| 新增面板功能 | 前端页面放 `src/pages/`，后端加一个 `commands/` 下的 Tauri Command 并在 `lib.rs:288` 的 `generate_handler!` 注册。**前端一律通过 `lib/runtime.ts` 的 `invoke` 调用**，不要直接 `@tauri-apps/api` |
| 改数据库 schema | 新增 `migrations/037_*.sql`，**不要改已发布的迁移**；启动时自动执行且自动备份 |
| 新增内置安全规则 | `security/rules.rs`（播种进 `security_builtin_rules`，用户可覆盖级别） |
| 加后台定时任务 | 参考 `health_probe.rs` 的 `run_probe_loop` 形态，在 `web_server.rs:121-146` 与 `lib.rs:247-272` **两处**都要 spawn |

最后一条要特别注意：**桌面与 headless 是两个入口，后台任务的 spawn 代码是重复的**。只改一处会导致"桌面有、Docker 没有"或反之的诡异差异。

---

## 24. 与文档的不一致，以及发现的问题

### 24.1 `AGENTS.md` 已滞后于代码（建议同步）

| `AGENTS.md` 的说法 | 实际 |
|:---|:---|
| 版本 0.2.9（第 7 行） | **0.3.1**（`package.json` / `Cargo.toml` / `tauri.conf.json` 三处一致） |
| 迁移 001–027（第 67 行） | **36 个**（`001`–`036`） |
| 测试「约 690 个」（第 114 行） | **931 个** `#[test]` / `#[tokio::test]` |
| 「无 Zustand/TanStack——依赖已确认为死依赖并移除」（第 170 行） | 根 `src/` 确实没有，但 **`web/` 子包依赖 `zustand` ^5 与 `@tanstack/react-query` ^5**（`web/package.json`） |

`AGENTS.md` 第 76 行自己也提醒了"README 的项目结构可能滞后于代码"，现在轮到它自己需要更新。README 的版本历史是准确的（含 v0.3.1 changelog）。

### 24.2 代码问题：`mark_probe_ok` 缺少 `.await`（**被动反哺实际未生效**）

`src-tauri/src/endpoint_executor/driver.rs:218-224`：

```rust
crate::core::attempt::AttemptResult::Success(_) => {
    // 被动反哺（C-04）：真实请求传输成功 → 立即恢复渠道的主动探测健康标记
    repo.mark_probe_ok(channel_id);          // ← 缺 .await
    repo.record_channel_mode_success(channel_id, endpoint, is_stream).await
}
```

同样的写法在 legacy 直连路径里也出现了一次（`core/proxy.rs:395`）：

```rust
// 被动反哺（C-04）：真实请求成功 → 恢复该渠道的主动探测健康标记
// （best-effort，与日志/配额递增同级的旁路更新）。
repo.mark_probe_ok(&channel.id);          // ← 同样缺 .await
```

而 `Repository::mark_probe_ok` 是异步方法（`db/repository.rs:98` `pub async fn mark_probe_ok`）。缺少 `.await` 意味着返回的 `Future` 被直接丢弃、**函数体从未执行**，因此：

- 注释里承诺的"真实请求成功即恢复渠道探测健康标记（C-04 被动反哺）"**没有实际发生**——新执行器与 legacy 路径**两条路都失效**，不是单点问题；
- 渠道探测失败后，只能等下一次主动探测（默认 300s 一轮）才能恢复，而不是被真实成功流量立即救回；
- `driver.rs` 同一 `match` 分支的第二行 `record_channel_mode_success` 有 `.await`，说明是遗漏而非有意——**同一处注释下的两行，一行写对一行写错**，且两处调用点措辞一致，属于从同一片段复制传播的缺陷。

因为 `Future` 带 `#[must_use]`，编译器应会给出 `unused_must_use` 告警。鉴于 `AGENTS.md:124` 要求"clippy 告警保持为零"，建议在本仓库构建时留意这一条告警是否被 `#[allow]` 掉或已进入 CI 的豁免名单——如果 CI 是干净的，那这里可能还有额外的告警抑制需要一并复查。

### 24.3 行为特征：SPA fallback 让"任意路径都返回 200"

`server/static_assets.rs:32-35` 对未命中的资源路径**回退到 `index.html` 且状态码 200**：

```rust
let (served_path, asset) = match WebAssets::get(path) {
    Some(content) => (path.to_string(), Some(content)),
    None => ("index.html".to_string(), WebAssets::get("index.html")),
};
```

这是 SPA（客户端路由）的标准做法，但排障时要意识到后果：**`GET /.env`、`GET /config.json` 这类探测也会拿到 200 + 1.7MB 的 index.html**。实测反代访问日志里全是 200，看起来像"这些敏感文件存在"，实际上只是回退。判断某个路径是否真的存在，不能只看状态码，要看 `Content-Type` 和响应体。

### 24.4 已知未完成项（代码内自述）

- **视觉能力路由**：当前对"请求含图片块而上游以 400 拒绝"只在错误信息里追加诊断提示（fail-open），**不改变路由行为**；基于渠道能力标记（`supports_vision`）的故障转移跳过是长期方案（README「安全边界」末段）。
- **`usage_source` 实时路径未写入**：见第 12 节末。

---

## 附：一分钟速览

```
下游（Chat / Responses / Messages）
   │
   ├─ [API Key 鉴权 + 宽松 CORS]
   ├─ 安全扫描（fail-closed 预算）
   ├─ 语义缓存（exact 哈希 / 向量余弦≥95%，命中回 X-Cache: hit 并记账）
   │
   └─ RoutePlan 生成（渠道 + Auth 账号 → 按 priority 分档、档内按 weight 无放回加权抽样）
         │
         └─ AttemptFlow 逐个候选尝试
              ├─ 协议转换（9 条方向矩阵，经 protocol_boundary 防腐层）
              ├─ 上游 SSE 字节级重组（CJK 安全）
              ├─ 失败分类（6 类）→ 组内换候选 / 组间降级（仅 Retryable、EndpointUnsupported）
              └─ 提交后禁止重试（stream_supervisor）
                    │
                    └─ request_logs 落账（含 trace_id、失败分类、token 用量）
                         └─ stream_segments 溢出表（流式大内容 + Responses 续传锚点）
```

两条健康信号独立运作：**主动探测**（渠道级，300s 一轮，失败沉底）与**模式健康**（渠道×端点×流式，被动记录，窄条件触发冷却）。

配套文档：[`deployment-guide.md`](deployment-guide.md)（部署与运维）。


