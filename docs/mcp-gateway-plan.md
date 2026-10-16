# MCP 网关实施计划（总体规划）

> 2026-10-16 · 前置分析见 `docs/mcp-gateway-feasibility.md`。本文档是可执行的分期实施计划，覆盖后端、前端 UI（含仪表盘/日志/使用/设置）、数据库、安全与发布全链路。

## 零、目标形态（一句话）

WaLiAPI 从「LLM API 网关」升级为「LLM + 工具双网关」：在【服务】侧配置工具连接器（外部 MCP / HTTP / CLI / 串口 / 脚本），经统一 `/mcp` 端点暴露给 AI 客户端；在【使用】侧测试调试并一键接入客户端；仪表盘与日志完整呈现工具调用维度。

## 一、总体节奏（4 个里程碑）

| 里程碑 | 内容 | 核心交付 |
|---|---|---|
| M1 基座 | ToolProvider 重构 + 连接器数据模型 | 内置 KB/Wiki 工具迁入新框架，行为不变（回归保障） |
| M2 连接器 P0 | MCP 代理 + HTTP/OpenAPI 导入器 | 服务页「MCP 网关」上线，可接外部 MCP 与 OpenAPI 软件 |
| M3 使用闭环 | MCP 测试台 + 客户端一键接入 + 日志/仪表盘/用量 | 端到端体验完整，可对外宣传使用 |
| M4 扩展 P1 | CLI + 串口（Modbus）+ 脚本连接器 | 桌面端本机执行能力；GUI 自动化立项评估（P2，单列） |

依赖关系：M1 → M2 → M3 → M4 严格串行（M3 的 UI 依赖 M2 的数据源；M4 的连接器只是往 M1 框架里加 Provider，不阻塞 M3）。

## 二、M1 基座重构（ToolProvider + 数据模型）

### 后端

1. 新增 `services/mcp/tool_provider.rs`：
   ```rust
   #[async_trait]
   pub trait ToolProvider: Send + Sync {
       fn id(&self) -> &str;                       // "builtin-kb" / "connector:<id>"
       async fn list_tools(&self) -> Vec<ToolDef>; // name/description/inputSchema
       async fn call_tool(&self, name: &str, args: Value) -> ToolResult;
       fn visible_for(&self, scope: &AuthScope) -> bool; // scoped 鉴权过滤
   }
   ```
2. 改造 `handlers.rs`：`get_tools()` 与巨型 `match` 拆为 `BuiltinKbProvider`（KB 13 工具）与 `BuiltinWikiProvider`（Wiki 16 工具）；`dispatch_jsonrpc_async` / `dispatch_scoped` 改为遍历 provider 注册表（`AppState` 持有 `Vec<Arc<dyn ToolProvider>>`）。**对外 JSON-RPC 行为零变化**，以现有测试 + 手工对拍保障。
3. 新增迁移（顺延编号，如 `045_mcp_connectors.sql`）：表 `mcp_connectors(id, name, connector_type, config JSON, status, created_at, updated_at)` + `mcp_connector_tools(connector_id, tool_name, description, input_schema JSON, enabled, override_config JSON)`（缓存上游工具列表 + 逐工具启停开关）。
4. `AppState` 增加 connector 注册表缓存；连接器增删改时热更新（无需重启服务）。
5. 审计接入：所有连接器工具调用参数先过 `security/scanner`（策略复用现有 只审计/警告/脱敏/阻断）。

### 前端

- `src/types` 增加 Connector 类型；`src/lib/api.ts`（走 runtime.ts 命令层）增加 CRUD 命令；新增 Tauri commands `commands/connector.rs`。
- 侧边栏「服务」组新增「MCP 网关」入口（此里程碑先放占位页）。

### 验收

- 全量测试通过；`/mcp` 对现有 29 工具的行为与重构前逐一对拍一致（脚本对拍 tools/list + 每工具一次调用）。

## 三、M2 连接器 P0（MCP 代理 + HTTP/OpenAPI）

### MCP 代理连接器（外部 MCP Server）

- `services/mcp/connectors/mcp_client.rs`：自实现 JSON-RPC client，P0 仅支持 Streamable HTTP / SSE 上游（stdio 后续）；连接时 `initialize` → `tools/list` 缓存入 `mcp_connector_tools`；`call_tool` 转发。
- 配置项：URL、鉴权（Bearer/自定义 header）、超时、启用工具白名单、命名前缀（避免与内置工具撞名，如 `figma_`）。
- 健康探测：复用渠道健康探测模式（`last_probe_at/ok/latency`），列表页显示连通状态。

### HTTP/OpenAPI 连接器

- `connectors/http_connector.rs`：两种录入方式——
  1. **OpenAPI 导入**（主打）：粘贴/上传 spec → 解析 paths/operations → 自动生成工具（operationId 为工具名，parameters/requestBody 映射 inputSchema）；前端提供导入预览与逐操作勾选。
  2. **手工模板**：URL 模板（`/pets/{id}`）+ method + 参数映射 + 认证方式（Bearer/QueryKey/自定义 header，复用 `AuthScheme` 概念）。
- 安全约束：SSRF 防护（禁 localhost/内网地址可配置豁免——注意本地软件恰恰是 localhost，默认**允许** 127.0.0.1、默认拒绝私网其他段）；响应体大小上限；敏感 header 不回显。

### UI（服务页 · MCP 网关）

- 列表页：连接器卡片（类型图标、状态、工具数、延迟、启停开关）。
- 表单抽屉/向导：选类型 → 按类型出配置表单 → 「拉取工具」预览 → 逐工具勾选启停 → 保存并测试。
- 复用现有组件风格（ChannelForm 的抽屉 + DraftTestModal 的测试模式）。

### 验收

- 接入 1 个真实外部 MCP（如 Playwright MCP）+ 1 个 OpenAPI 服务，Claude Code 端到端调用成功；SSRF/超时/撞名用例单测覆盖。

## 四、M3 使用闭环（测试台 + 客户端接入 + 数据可视）

### 使用页（新增「MCP 测试台」Tab 或子页）

1. 连接信息区：`/mcp` 端点 URL、当前鉴权模式（MCP_TOKEN / API Key scoped）、协议版本。
2. 工具浏览器：聚合展示全部 provider 的工具（内置 + 各连接器），按来源分组，显示 schema。
3. 调用调试器：选工具 → 表单化填参（由 inputSchema 生成）或 raw JSON → 执行 → 结果/错误面板 + 耗时；调用历史（本地 state 即可）。
4. 客户端一键接入：生成 `claude_desktop_config.json` / `.mcp.json`（Claude Code、Cursor、Codex 等格式）代码片段 + 复制按钮；桌面端支持直接写入文件（Tauri 文件对话框，`#[cfg(feature = "desktop-ui")]`）。

### 日志页

- `request_log` 扩展（新迁移）：`entry_kind`（`llm` / `mcp_tool`）或独立表 `mcp_tool_calls(id, ts, connector_id, tool_name, args_digest, status, latency_ms, error, api_key_id, client_info)`——**推荐独立表**，参数全文不入库（只存摘要/截断），避免审计日志泄漏敏感参数。
- 日志页新增筛选维度「工具调用」，行详情展示工具名、来源连接器、耗时、状态、参数摘要。

### 仪表盘（Dashboard）

- 新增「MCP 工具调用」统计卡：今日调用量、成功率、Top 工具（横条图）、按连接器分布（环图）。
- 复用现有趋势图组件，加「工具调用」时间序列（与 Token 趋势同布局、胶囊切换）。
- 热力图不动（Token 专属）。

### 用量页（Usage）

- 用量聚合增加 MCP 维度：按工具/连接器统计调用次数与耗时（工具调用无 Token，不进 Token 汇总，单独区块）。

### 设置页（Settings）

- MCP 网关区块：默认启停策略（本机执行类工具默认关）、SSRF 白名单、审计策略绑定、客户端接入配置的默认导出路径。

### 验收

- 从「接入一个 HTTP 软件 → 测试台调试 → 写入 Claude 配置 → Claude 实际调用 → 日志/仪表盘可见」全流程演示通过。

## 五、M4 扩展（CLI / 串口 / 脚本）

| 连接器 | 实现要点 | 门控 |
|---|---|---|
| CLI | `tokio::process::Command`；工作目录、环境变量、超时、stdout/stderr 截断、退出码→isError | `desktop-ui` 门控（headless 禁用） |
| 串口 | `serialport` crate + Modbus RTU（读保持寄存器/写寄存器等常用功能码）；端口扫描、波特率等参数表单 | `desktop-ui` 门控；独占锁提示 |
| 脚本 | 用户/内置提供 AppleScript / JSX / PowerShell 脚本段，包装为单工具；内置驱动库（PS、Word OOXML 等常见脚本） | `desktop-ui` 门控 |
| GUI 自动化（P2） | AppleScript/UIA 通道先做；视觉兜底不承诺 | 实验特性开关，单独立项评估 |

UI：连接器类型下拉追加对应项，表单按类型渲染；本机执行类在列表页加「本机执行」徽标 + 二次确认。

## 六、横切事项（贯穿所有里程碑）

- **版本与发布**：版本号四处同步（package.json / Cargo.toml / tauri.conf.json / Cargo.lock）；CHANGELOG 每里程碑一条；文档更新 README + AGENTS.md（新模块、新迁移、新命令）。
- **测试纪律**：`cd src-tauri && cargo test` 全量通过、`cargo fmt`、clippy 零新增；迁移测试走内存 SQLite；前端 `pnpm build`（tsc strict）通过。
- **安全清单**：连接器凭证加密存储（复用渠道 api_key 的存储方式）；审计引擎覆盖所有工具参数；本机执行默认关闭；scoped API Key 白名单机制扩展到连接器工具（`QUERY_TOOLS` 模式推广为 provider 级声明）。
- **文档**：`docs/` 下连接器开发指南（如何新增 Provider）、用户使用指南。

## 七、里程碑建议排期

- M1：1 个迭代（重构 + 对拍，风险最高的是 handlers.rs 拆分）
- M2：1-1.5 个迭代（两个连接器 + 导入器 UI）
- M3：1 个迭代（纯前端为主 + 日志表）
- M4：1-2 个迭代（三个连接器可拆开各自发布）
- GUI 自动化：单独立项，不进主线排期
