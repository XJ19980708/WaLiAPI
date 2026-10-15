# WaLiAPI 可观测性增强 · 整体分析与设计

> 版本 v1.0 · 2026-10-15 · 状态：设计稿（待评审）
>
> 目标：参考 magpie（本地 LLM 网关，Go + vanilla JS）的成熟实践，为 WaLiAPI 补齐三大能力——
> ① 请求日志的**分类 token 统计 + 首字耗时/整体耗时**；② **仪表盘**呈现这些指标；③ **网关路由动态图**（调用方+Key → WaLiAPI 路由 → 渠道/映射）。

---

## 1. 现状分析

### 1.1 已有能力（调研确认）

| 能力 | 现状 | 位置 |
|---|---|---|
| prompt/completion/total/cached tokens | ✅ 已有 | `request_logs` 表、`extract_usage` |
| 总耗时 `duration_ms` | ✅ 已有 | 同上 |
| 渠道名/模型名/Key 名/协议/codec | ✅ 已有 | 016 迁移字段 |
| 失败分类 `failure_class` / `is_retry` | ✅ 最终结果级 | driver.rs |
| 小时级四维聚合 `usage_stats` | ✅ 已有但前端未消费 | 046 迁移 |
| token 估算 | ✅ tiktoken cl100k，整包估算 | `endpoint_executor/estimate_usage.rs` |

### 1.2 核心缺口（本次要补齐）

| # | 缺口 | 说明 |
|---|---|---|
| G1 | **首字耗时 TTFT 无字段** | 流式路径有首帧超时与 `stream_committed`，但未记录首字节/首内容时间戳 |
| G2 | **按消息角色分类的 token 拆分** | 估算与实测均不区分 system / tools / user / injected / assistant / tool_result |
| G3 | **per-attempt 调度明细** | 重试/fallback 中间尝试只写 tracing，仅最终行 `is_retry` 布尔 + last_failure |
| G4 | 仪表盘无分类 token、TTFT、速度等指标 | Dashboard 只有 6+6 卡片、模型分布、token 趋势 |
| G5 | 无路由拓扑可视化 | 完全缺失 |
| G6 | `started_at` 列存在但未填充 | 030 迁移已建列 |
| G7 | `usage_source` 实时路径未写入 | 仅 repair 路径在写 |

### 1.3 magpie 可借鉴要点（摘要）

- **分类体系**：`system / tools / memory(注入) / files / results(工具结果) / chat`，user 文本用"注入前缀列表"（`<system-reminder>`、`<environment_context>`、task-notification 等）区分真实用户输入与注入内容；"轮次"由真实用户发言触发。
- **估算 + 校准**：请求进入时按 ASCII 4 字符/token、CJK 1 字符/token 快速估算并分类记账；上游响应到达后用厂商真实 `input tokens` 计算 `scale = counted/est`，**等比缩放**每个分类，差额补给最大分类。估算先行、事后校准，对应参考图"估算合计 ≈12.4k / 实际 14.5k"。
- **只存摘要不存正文**：分类结果只存名字/大小，正文走内存 keepBody + 按需拉取接口，避免日志膨胀。
- **Route 模型**：一次请求一条 Route，内含 `Tries[]`（每次尝试的 model/status/millis/TTFT/FirstText/失败原因/冷却）与 `Order[]`（被权衡过的候选渠道及落选原因），天然是路由动画的数据源。
- **路由可视化**：三栏布局（调用方 → hub → 账号/渠道），纯手写 SVG 贝塞尔连线 + `getPointAtLength`/`requestAnimationFrame` 粒子飞行动画；failover 过程可回放；尊重 `prefers-reduced-motion`。
- **速度解码**：用 TTFT/FirstText/Flow 区分"真流式"与"一次性吐出"，避免假 tok/s。
- **零图表库**：全部自绘 SVG/div，几十行一个组件。

---

## 2. 总体设计

### 2.1 数据模型（三层）

```
┌─────────────────────────────────────────────────────────────┐
│ 层1 request_logs（现有表，增列）                              │
│   + ttft_ms INTEGER        首内容耗时（流式首字节/非流式≈首包）│
│   + started_at 填充        （G6，补写路径）                   │
│   + attempts INTEGER       尝试次数（>1 才有意义）            │
│   + prompt 分类列（见层2，此处只放冗余汇总）                   │
├─────────────────────────────────────────────────────────────┤
│ 层2 log_prompt_parts（新表，1:N）                             │
│   log_id / kind / items / est_tokens / tokens / pct          │
│   kind ∈ system|tools|injected|files|results|chat|assistant  │
├─────────────────────────────────────────────────────────────┤
│ 层3 log_attempts（新表，1:N）                                 │
│   log_id / seq / channel_id / channel_name / model /         │
│   status_code / failure_class / error_message /              │
│   queued_ms / millis / ttft_ms / is_final                    │
└─────────────────────────────────────────────────────────────┘
```

- 层2 对应参考图"上下文浏览器"的分类行（系统提示词/工具定义/用户消息/注入内容/技能注入/助手消息/工具结果）。
- 层3 对应 magpie 的 `Route.Tries`，是路由动画与"每次尝试明细"的数据源。

### 2.2 分类 token 统计（G2）—— 后端

新增 `src-tauri/src/endpoint_executor/prompt_classify.rs`：

1. **分类规则**（对齐 magpie，适配 WaLiAPI 三协议）：
   - `system`：`role=system/developer` 消息、Responses 的 `instructions`；
   - `tools`：`tools[]` 定义序列化后计数；
   - `injected`：user 文本命中注入前缀列表（`<system-reminder>`、`<environment_context>`、`<task-notification>`、`The TodoWrite tool`…，列表放常量便于维护）；
   - `files`：tool_result 中由文件读取类工具产生（按工具名启发：read/file/fetch…）；
   - `results`：其余 tool_result；
   - `chat`：真实 user 消息 + assistant 消息；
   - 每类下可再挂 `items[]`（名称、条数、tokens），对应参考图"工具定义 27 项 ≈6.8k"。
2. **估算器**：复用现有 tiktoken cl100k 单例；分类计数在遍历 messages 时顺手完成（单遍扫描）。
3. **校准**：上游 2xx 且真实 usage 到达时，`scale = real_prompt / est_prompt`，等比缩放各分类 tokens，差额补给最大分类；`usage_source` 同步写入 `upstream`/`estimated`（顺带修 G7）。
4. **存储**：估算+校准后的 7 行分类写 `log_prompt_parts`；不存正文（正文沿用现有 `request_body`/`stream_segments` 策略）。

### 2.3 TTFT 与耗时（G1/G6）

- 流式：`stream_supervisor.rs` 首个含内容的 SSE 事件到达时刻 − 请求开始时刻 = `ttft_ms`；现有 `StreamTimeouts.first_frame` 处已经有首帧判定点，顺手打点即可。
- 非流式：响应首包到达即记 `ttft_ms`（≈连接+处理时间，价值较低但保持字段统一）。
- `started_at`：进入 driver 时写入（补 G6）。
- 失败/取消行：能取到多少记多少，无则 NULL。

### 2.4 per-attempt 调度明细（G3）

- `plan_executor.rs` / `attempt.rs` 中每次物理尝试完成（成功或失败）时向 `log_attempts` 追加一行；最终成功的尝试 `is_final=1`。
- 失败冷却、排队时间（`queued_ms`）一并记录，供路由图展示"落选原因"。

### 2.5 新增查询与命令

```
get_log_prompt_parts(log_id)       → 层2 数据（日志展开行渲染分类条）
get_log_attempts(log_id)           → 层3 数据（尝试时间线）
get_log_stats 扩展                 → 增加 avg_ttft_ms、p50/p90 ttft、avg tok/s
get_dashboard_stats 扩展           → 增加 avg_ttft、cache 命中率、分类 token 汇总（近24h）
get_context_trend(session/trace)   → 轮次趋势点（时间、tokens、cache）→ 趋势图
```

全部经 `commands/` 的 `#[tauri::command]` + `*_impl` 模式导出，前端走 `src/lib/api.ts` 域分组包装，不直接调 Tauri API（遵守 runtime.ts 约定）。

---

## 3. 前端设计

### 3.1 日志页增强（LogsPage）

参考图 1 的"上下文浏览器"：

1. **表格新增列**：TTFT（`ttft_ms`，1.2s）、速度（tok/s，可开关）；
2. **展开行新增"上下文构成"卡片**：
   - 顶部横向堆叠条（按分类着色，hover 显示 tokens/占比）；
   - 下方分类明细行：色块 + 分类名 + 条数 + `+Δ` + ≈tokens + 百分比（估算态）或真实 tokens（校准态）；
   - 底部一行：实际 tokens、输出 tokens、缓存命中率、TTFT、总耗时；
3. **尝试时间线**：展开行内再渲染 `log_attempts`（每次尝试一行：渠道、状态、耗时、失败原因），fallback 过程一目了然。

### 3.2 仪表盘增强（DashboardPage）

1. 新增卡片：**平均首字耗时**、**平均输出速度**（tok/s，用 magpie 的真流式判定防假速度）；
2. **Token 趋势图升级**：现自绘 SVG 堆叠柱状图已具备三序列（input/output/cached），增加第四序列"缓存写"与 TTFT 折线（双 Y 轴）；
3. **分类 token 构成图**：近 24h 各分类 token 占比堆叠条（数据来自 `log_prompt_parts` 聚合），点击可跳转日志页带筛选；
4. 模型分布表增加 TTFT 列。

### 3.3 网关路由动态图（新页面 RouteMapPage，G5）—— 完整设计

#### 布局（三栏 + 连线层，参考 magpie 舞台结构）

```
┌────────────┐      ┌──────────────┐      ┌──────────────────────┐
│ 调用方(左)  │      │   WaLiAPI    │      │ 渠道/映射(右)         │
│            │      │   (hub)      │      │                      │
│ ● Claude   │──────▶│  路由/调度    │──────▶● DeepSeek  (映射映射) │
│   sk-...ab │      │  协议转换     │      │● OpenAI   gpt-4o     │
│ ● Codex    │──────▶│  安全审计     │      │● Claude   主/备       │
│   sk-...cd │      │  故障切换     │      │● Ollama   本地        │
│ ● 脚本 xx  │      │              │      │○ (冷却中·灰)          │
└────────────┘      └──────────────┘      └──────────────────────┘
        └──────── SVG wires 连线层（贝塞尔）+ 粒子层 ────────┘
```

- **左栏节点 = API Key（调用方）**：显示 Key 名称（`api_key_name`）+ 备注/调用方标识；按 Key 聚合请求量徽标。
- **中间 hub = WaLiAPI 网关**：单节点，环上显示当前 QPS、今日请求、运行状态；点击展开协议入口徽标（chat/responses/messages）。
- **右栏节点 = 渠道**：渠道名 + provider 图标 + 模型映射摘要（`model → upstream_model` 前若干条）；**状态着色**：健康（正常色）/ 冷却中（灰 + 剩余秒数）/ 探测中（黄）；数据来自 `channel_mode_health` 与渠道列表。
- **边**：Key→hub（该 Key 最近的请求）、hub→渠道（route group 实际命中的渠道，粗细 ∝ 流量）。

#### 交互与动效

1. **实时粒子**：每个进行中的请求在 Key→hub→渠道的路径上飞一个圆点（颜色随 Key）；响应成功后反向飞回。SSE 事件（复用 `/admin/api/events` 桥）驱动，事件需后端在 driver 关键节点 emit：`request_start / attempt_start / attempt_result / request_done`。
2. **failover 回放**：请求失败时粒子飞到 A 渠道变红坠落，再飞向 B 渠道——把 `log_attempts` 的历史也做成可回放（选中一条历史日志，逐步播放其 tries）。
3. **历史回放条**：底部时间轴，可拖动选择过去 N 分钟，倍速播放。
4. **静态模式**：`prefers-reduced-motion` 或关闭动效开关时退化为静态拓扑 + 数值标注。
5. **点击节点**：左节点 → 筛选该 Key 的日志；右节点 → 渠道详情（健康、命中率、平均 TTFT）。

#### 技术选型

- **沿用 WaLiAPI 前端栈（React 19）自绘 SVG**，不引入 reactflow/d3：节点为 absolutely-positioned div 卡片，连线为铺底 `<svg>` 的 `<path>`（三次贝塞尔），`ResizeObserver` + `getBoundingClientRect` 计算端点，`requestAnimationFrame` 驱动粒子（`getPointAtLength`）。与 magpie 方案同构，量级可控（预计 ~600 行组件）。
- 新增事件经 `event_bridge.rs` 统一出口（桌面 Webview 与 Web SSE 已统一），前端 `runtime.listen` 订阅，零新增通道。

---

## 4. 实施计划（分四期，每期可独立交付）

| 期 | 内容 | 交付物 | 主要文件 |
|---|---|---|---|
| P1 观测打点 | TTFT/started_at/attempts 落库；新迁移 033+（`log_prompt_parts`、`log_attempts`、request_logs 增列）；分类器 + 校准 | 日志数据完备 | migrations、driver.rs、stream_supervisor.rs、prompt_classify.rs（新）、plan_executor.rs |
| P2 日志 UI | 日志页 TTFT/速度列、上下文构成卡片、尝试时间线 | 参考图1 落地 | LogsPage.tsx、api.ts、commands/log.rs |
| P3 仪表盘 | 新卡片、趋势图升级、分类构成图 | 参考图2 对应能力 | DashboardPage.tsx、commands/stats.rs |
| P4 路由图 | RouteMapPage + SSE 事件 + 粒子动画 + 回放 | 参考图3/路由舞台 | RouteMapPage.tsx（新）、event_bridge.rs、driver.rs |

### 风险与对策

- **日志膨胀**：分类表每请求 ~7 行、attempts 通常 1-2 行，量级可控；`detail_level=basic` 时分类仍记账（只含大小不含正文），保留期清理与主日志级联（`ON DELETE CASCADE`）。
- **估算精度**：claude/gemini ±10-20%，校准后分类间相对比例可靠；UI 明示"估算 ≈"与"实际"两态（对齐参考图）。
- **动画性能**：粒子按帧合并、上限（如同时 ≤30 个）、页面不可见时暂停。

---

## 5. 待确认决策点

1. 分类名是否采用参考图中文口径（系统提示词/工具定义/用户消息/注入内容/技能注入/助手消息/工具结果）？技能注入（skills）在 WaLiAPI 无对应概念，建议并入 injected。
2. `log_attempts` 是否需要记录每次尝试的请求体摘要（便于排查）？建议只记 HEAD 信息，正文仍走现有策略。
3. 路由图是否需要覆盖 Auth 账号（`upstream_type=auth_account`）节点？建议渠道与 Auth 账号并列展示为右侧两类节点。
4. TTFT 口径：首个 SSE 字节 vs 首个可见内容 token？建议用**首个可见内容**（与 magpie FirstText 一致，避免把心跳/注释帧算进去）。
