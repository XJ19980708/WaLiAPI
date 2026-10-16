# MCP 网关可行性分析

> 2026-10-16 · 目标：在【渠道】中接入外部 MCP 服务 / HTTP 接口 / 串口 / 本地软件，统一转换为 MCP 工具对外暴露；在【使用】中提供 MCP 测试台与客户端配置一键接入。

## 一、结论

**整体可行，且与现有架构契合度高。** 建议按「连接器类型」分层实现，优先级：

| 优先级 | 来源类型 | 可行性 | 核心技术 | 风险 |
|---|---|---|---|---|
| P0 | 外部 MCP 服务（代理汇聚） | 高 | 自实现 MCP client（JSON-RPC over HTTP/SSE），已有手写 JSON-RPC 经验 | 上游不可用时的降级策略 |
| P0 | HTTP/REST 接口 | 高 | reqwest + 请求模板映射为 tool（URL/method/参数 schema） | 无 schema 时需手填参数定义 |
| P1 | 本地软件 CLI | 高 | `tokio::process::Command`（已有 git importer 先例） | 长任务超时、输出裁剪 |
| P1 | 串口 RS-232/485 | 中高 | `serialport` crate + 帧协议配置（Modbus 等） | 独占占用、帧协议差异大，建议先支持 Modbus RTU |
| P2 | 纯 GUI 软件操作 | 中（分平台） | 见下文第四节 | 脆弱、授权繁琐、工作量大，建议作为实验特性 |

## 二、「AI 操作软件」的四条实现路径

AI 操作一个软件，本质是把该软件的能力暴露为 MCP tool。按软件的可编程程度分四条路径：

1. **软件自带 MCP Server**（如 Figma、GitHub、Playwright 等）：网关作为 MCP client 连接并转发，零改造，最标准。
2. **软件有 CLI**（如 ffmpeg、adb、pandoc）：每个命令包装为一个工具，参数即命令行参数。稳定可靠，是"操作本地软件"的最务实形态。
3. **软件有本地 HTTP API**（很多桌面软件内嵌 localhost 端口，如 Obsidian、Joplin）：REST 映射为工具，同 P0 的 HTTP 连接器。
4. **软件只有 GUI、无任何接口**：需要系统级 UI 自动化——
   - macOS：AppleScript / Accessibility API（部分软件支持 AppleScript 字典，如 Finder、Notes、Excel for Mac）；
   - Windows：UI Automation（UIA）/ PowerAutomate；
   - 通用兜底：截图 + 视觉定位 + 模拟点击（computer-use 路线），效果依赖视觉模型，脆弱且慢。
   - **结论：可以实现，但建议放到最后做，且先只做 macOS AppleScript 通道**；优先引导用户走路径 1-3。

## 三、后端改动面（基于代码调研）

- **动态工具注册表（核心前置重构）**：现 `services/mcp/handlers.rs` 的 29 个工具硬编码在 `get_tools()` + 巨型 `match` 中（单文件 3300+ 行）。需抽出 `ToolProvider` trait（`list_tools()` / `call_tool()`），内置 KB/Wiki 工具改为其第一个实现，各连接器作为后续实现。这是唯一较大的重构项。
- **连接器执行层**：新增 `services/mcp_gateway/`（或 `connector/`）模块：
  - `mcp_client.rs`：连外部 MCP（initialize/tools list/tools call 三个方法即可）；
  - `http_connector.rs`：请求模板（URL 模板 + 参数映射 + header）；
  - `cli_connector.rs`：进程执行 + 超时 + 输出截断；
  - `serial_connector.rs`：serialport + Modbus RTU 帧；
  - `desktop_connector.rs`（P2）：AppleScript / UIA。
- **数据模型**：连接器配置可复用 `channels` 表（`config` JSON 列 + `channel_type` 扩展），或独立 `mcp_connectors` 表（推荐，避免与 LLM 渠道的 priority/weight 语义混淆）。新增迁移顺延编号。
- **鉴权与安全**：沿用 `require_mcp` 两档鉴权（MCP_TOKEN 全量 / API Key scoped 白名单）。GUI/CLI/串口类工具涉及本机执行，必须默认关闭、逐工具启用，并接入现有安全审计引擎（security/scanner）对参数做风险扫描。
- **使用面**：
  - 新增「MCP 测试台」页面：tools list 浏览、单工具调用调试、结果预览；
  - 「一键接入」：生成各客户端配置片段（`claude_desktop_config.json`、`.mcp.json`、Cursor 格式），支持复制或直接写入本机配置文件（桌面端走 Tauri 文件对话框）；
  - Usage：MCP 工具调用计入现有 request_log（新增 tool_name 维度）。

## 四、前端改动面

- ChannelForm 是 preset 数据驱动的（ProviderDropdown 吃 presets 数组），但协议列表 `PROTOCOLS = ["openai","anthropic","ollama"]` 硬编码于 `ChannelForm.tsx:26`；连接器类型建议用独立的「连接器」分类 UI，而不是塞进 LLM 渠道表单。
- 图标/分组：`src/lib/constants.ts`。
- 所有前后端交互走 `src/lib/runtime.ts` 统一命令层（项目约定）。

## 五、价值评估

- **定位升级**：从「LLM API 网关」升级为「LLM 网关 + 工具网关」，补齐 Agent 基础设施最后一块：模型可以代理，工具也可以代理。
- **差异化**：市面 MCP 网关（如 mcp-gateway 类开源项目）多只做 MCP→MCP 代理；同时覆盖 HTTP/串口/本地软件的很少。串口方向切入工业/硬件场景（PLC、传感器、仪器控制）几乎无竞品。
- **场景**：个人——让 Claude/Cursor 直接操作本机软件；团队——headless 部署后统一管控工具访问（鉴权、审计、脱敏复用现有能力）；工业——串口设备 AI 运维。
- **成本**：P0+P1 约 3-4 个迭代周期（含 ToolProvider 重构）；P2 GUI 自动化单独评估。

## 六、风险与建议边界

1. GUI 自动化不建议承诺"所有软件都能操作"——无接口软件只能靠视觉方案，稳定性不可控。
2. 本机执行类工具（CLI/GUI/串口）与 headless 服务器部署天然冲突（Docker 内无 GUI/串口），应仅桌面端启用（`#[cfg(feature = "desktop-ui")]` 门控）。
3. MCP 协议目前手写实现，若要支持外部 MCP 的 stdio 传输，需补 stdio 子进程通道；建议 P0 仅支持 Streamable HTTP/SSE 上游。

## 七、能否「统一标准」接入，而不用逐个分析软件？

结论：**网关侧可以有一套统一框架，但「每个软件怎么被操作」没有万能标准——不过每条路径内部各有标准，按家族接入可以大幅摊薄成本。**

四个家族，按「每个软件需要多少定制」排序：

| 家族 | 统一标准 | 每软件定制成本 | 典型例子 |
|---|---|---|---|
| MCP Server | MCP 协议本身 | **零**（厂商已做好） | Figma、Playwright、GitHub |
| HTTP API | **OpenAPI/Swagger**：粘贴 spec 自动生成工具列表与参数 schema | 近零（有 spec 时）；无 spec 时手工填模板 | 各类 SaaS、内嵌 localhost 端口的桌面软件 |
| 脚本/自动化接口 | OS 级统一协议：macOS AppleScript 字典 / Windows UIA / Office JS API、ExtendScript(PS)、OOXML(Word) | 中——协议统一，但每个软件要写一段专属脚本/驱动 | Word（JS API/OOXML）、Photoshop（JSX/UXP）、Excel |
| 无接口 GUI | 无标准。兜底方案 = 视觉智能体（截图→视觉模型定位→模拟键鼠） | 单个软件近零，但**通用可靠性差**、慢、依赖视觉模型 | 微信个人版、老旧桌面软件 |

三个具体例子的落地方式：

- **微信收发消息**：个人微信无官方 API，开源方案（wxauto/WeChatFerry 等）基于 UI 自动化或内存 hook，稳定性与封号风险需评估；企业微信有官方 API（可作 HTTP 连接器接入）。**建议只接企业微信，规避风险。**
- **Photoshop 做图**：有官方脚本通道（ExtendScript/JSX、新版 UXP），可包装为 `ps_execute_script` / `ps_export_layer` 等工具，macOS 还可直接从 AppleScript 驱动。
- **Word 出文档**：三条路——本地 OOXML 直接生成（python-docx 思路，纯文件操作最稳）；Office JS API（需 Word 联网加载插件）；AppleScript（仅 Mac 版 Word）。推荐 OOXML 文件级操作，不经 GUI。

对网关设计的指导意义：

1. 连接器框架统一（一个 ToolProvider 接口 + 统一的工具 schema/审计/日志），**驱动按家族批量生产**：OpenAPI 导入器（贴 spec 自动出工具）值得做成 P0 特性，它把「接一个新 HTTP 软件」的成本压到接近零。
2. 脚本家族做一个「脚本连接器」：用户/内置驱动提供一段脚本（AppleScript/JSX/PowerShell），网关包装成工具。官方生态里已有大量现成脚本可搬运。
3. 视觉兜底（computer-use）不建议自研——可靠性工程量巨大；若真需要，等接入第三方视觉模型路线或直接不承诺。
4. 期望管理：宣传口径应为「有 MCP/OpenAPI/脚本接口的软件都能接，覆盖主流办公与创作软件；无接口软件不承诺」。

## 八、信息架构归属：渠道 vs 服务

结论：**配置管理放「服务」侧（新建「MCP 网关 / 连接器」页面），测试与一键接入放「使用」侧；不建议放进「渠道」。**

理由：

1. **流量方向相反**：渠道 = 出站（把请求转发到上游 LLM 供应商），核心语义是负载均衡/优先级/模型映射/故障切换；MCP 网关 = 入站（把本机与外部能力作为工具暴露给下游 AI 客户端）。复用渠道页会让 priority/weight/模型映射等表单项对连接器毫无意义，还得大量隐藏逻辑。
2. **代码归属一致**：现有 MCP 已是 `services/mcp`（Service trait + ServiceRegistry），KB/Wiki 同属服务域；连接器作为新的 ToolProvider 挂进同一 Service，产品 IA 与代码结构对齐，后续维护成本低。
3. **数据模型呼应**：连接器建议用独立的 `mcp_connectors` 表（不与 LLM 渠道共用 `channels` 表），与「服务」归属互相印证。
4. **「使用」侧不受影响**：MCP 测试台（tools list 浏览、单工具调试）、客户端配置一键写入（claude_desktop_config.json / .mcp.json / Cursor）放「使用」，与现有 API 使用/测试的心智一致。

折中方案（若坚持从渠道入口进）：渠道页顶部加「LLM 渠道 / 工具连接器」两个 Tab，底层完全独立。仅在「用户习惯从渠道页找一切接入配置」的既有心智很强时使用，长期仍建议独立页面。
