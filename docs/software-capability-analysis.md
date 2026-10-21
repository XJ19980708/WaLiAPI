# WaLiAPI「软件对接能力域」完整分析

> 目标：在现有 知识库（KB）/ Wiki / MCP 三个能力之外，新增**软件对接**能力——针对本地软件实现可控操作，并把操作能力以 MCP 工具形式暴露，供 AI Agent 调用。首期以「微信」为代表案例，同时预留「接口（HTTP API）」「串口」等后续能力域的扩展位。
>
> 状态：**P0/P1/P2/P3 已实现（2026-10-16）**，决策已定：macOS + Windows 双平台同步；桌面弹窗审批；注入类不进主线。
> 已落地：`053_software_instances` 迁移、`services/software/` 能力域（generic_cli + generic_http + **wechat（P2，macOS 实验性）** + **generic_mcp 聚合网关（P3）** 四连接器、白名单执行器、桌面弹窗审批、审计日志、**60s TTL 工具清单缓存**）、MCP `tools/list` 动态组装（`software__` 命名空间）、**本机软件扫描 + 勾选接入**（macOS /Applications、Windows Program Files、Linux desktop files，Ollama/qBittorrent/Jupyter/微信命中内置预设）、`commands/software.rs` 9 个管理命令、前端「服务 → 软件」Tab（连接器市场 / 扫描接入 / 实例管理 / 工具预览 / 调用日志）。scoped（API Key）模式天然不暴露软件工具。
>
> 待办：微信连接器实测调优（AppleScript 流程依赖具体版本界面）、Windows 微信（wxauto 外置方案）评估、P4 接口/串口深化。

---

## 一、现状盘点（基于当前代码）

### 1.1 MCP 工具体系现状

| 事实 | 位置 | 对本次设计的影响 |
|:---|:---|:---|
| 工具以 `serde_json::json!` 静态硬编码返回，约 26 个 | `src-tauri/src/services/mcp/handlers.rs` → `get_tools()` | 新增软件工具必须改为**动态组装** `tools/list` |
| 分发为字符串 `match tool_name` 大分支 | `handlers.rs` → `handle_tool_call` | 软件工具应走独立的子分发器，避免继续膨胀单文件 |
| API Key 受限模式靠 `QUERY_TOOLS` / `WIKI_QUERY_TOOLS` 两个硬编码白名单过滤 | `handlers.rs` → `dispatch_scoped` | 软件能力属于**高危能力**，scoped 暴露必须默认关闭、按软件粒度授权 |
| 每库/每项目一个 `mcp_enabled` 开关控制 MCP 可见性 | `kb.mcp_enabled` / `wiki.mcp_enabled`（SQL 过滤） | 软件对接沿用同一模式：**每个软件实例一个 `mcp_enabled`** |
| `/mcp` 鉴权：独立 MCP Token，降级走 API Key（限 POST、禁 SSE 会话） | `server/admin.rs` → `require_mcp` | 软件操作不建议走 API Key 降级路径，仅允许 MCP Token 全量模式使用 |

### 1.2 执行基础设施现状

- 代码中**没有**通用本地命令执行、GUI 自动化能力。仅三处受限进程调用：`git clone`（KB 导入）、`open/explorer/xdg-open`（打开目录）；`osascript` 只出现在安全扫描器的风险关键词规则里。
- Cargo 依赖中没有 enigo / accessibility / UIA 类自动化库；`tauri-plugin-shell` 已引入但未实际使用；`tokio::process` 可直接复用（importer.rs 已在用）。
- `security/scanner.rs` 的 25+ 规则会把命令执行类片段（`bash -c`、`osascript` 等）标为 Medium 风险——软件能力上线后需要**配套的白名单放行与审计策略**，否则自己扫自己会持续告警。

### 1.3 结论

软件对接是一个**全新能力域**：执行基础设施零复用，但「服务注册（Service trait / ServiceRegistry）→ 前端页面 → admin invoke → MCP 暴露 → mcp_enabled 粒度开关」这条管线模式是成熟的，照抄 KB/Wiki 的结构即可，成本集中在执行层与安全层。

---

## 二、本地软件可控性的技术路径全景

控制一个本地软件，按「侵入性从低到高、通用性从高到低」分五类路径：

### 路径 A：原生接口对接（首选，稳定）
软件自身提供本地 API：HTTP 端口（Ollama `11434`、Jupyter、qBittorrent WebUI）、CLI（`git`、`ffmpeg`、`brew`、`code`）、IPC/Socket（Docker unix socket、mpd）。
- **实现**：HTTP 用现成 reqwest；CLI 用 `tokio::process` 白名单执行器。
- **特点**：语义清晰、结果结构化、跨平台一致、几乎不碎。WaLiAPI 已有完整的 HTTP 客户端与并发基础设施，这类对接成本最低。
- **覆盖面**：开发者工具、下载器、数据库客户端、媒体服务、自研软件——覆盖面比想象大，但不含微信。

### 路径 B：系统脚本桥（次选，平台分叉）
macOS AppleScript / JXA（`osascript`）、Windows COM + PowerShell、Linux DBus。
- **实现**：`tokio::process` 调 `osascript -e` 等；Windows 用 `powershell -Command` + COM 自动化。
- **特点**：macOS 上 Music、Notes、Finder、Calendar、Terminal 等**原生应用支持度高**，一句话就能读写；但每个软件脚本字典不同，Windows/Linux 无法复用同一套脚本。
- **这是「通用解决方案」里性价比最高的一层**：一套执行器 + 按软件配置的脚本项目，就能覆盖 macOS 大量原生软件。

### 路径 C：UI 自动化（兜底，脆弱但通用）
模拟键鼠（enigo）+ 读屏（macOS Accessibility/AX API、Windows UIA、Linux AT-SPI）+ 截图 OCR。
- **实现**：macOS 需要用户授予「辅助功能」权限（`AXUIElement` 系列，可用 `accessibility` crate 或 objc bridge）；键鼠注入用 `enigo`。
- **特点**：理论上能操作任何有界面的软件，实际脆弱——依赖控件树结构和坐标，软件升级即碎；读屏质量决定上限。
- **定位**：作为 A/B 不适用时的兜底，工具粒度应做成「读界面树 / 点控件 / 输入文本 / 截图」这类**低层原语**，让 Agent 自己组合，而不是为每个软件硬编码点击序列。

### 路径 D：注入 / Hook（不建议做进主线）
DLL 注入、内存 hook、逆向私有协议（Windows 上的 WeChatFerry 等）。
- **特点**：能力最强（能拿到完整消息收发），但：违反软件用户协议、账号封禁风险由用户承担、随目标软件版本升级极易失效、有安全与法律风险。
- **结论**：**WaLiAPI 主线不内置**。可作为社区外挂连接器留接口，文档明示风险，责任归用户。

### 路径 E：MCP 聚合网关（免费的能力放大器）
社区已有大量现成 MCP Server（filesystem、playwright 浏览器、各类软件官方 MCP）。WaLiAPI 作为**MCP 出口网关**，把外部 stdio/SSE MCP Server 的工具**代理聚合**进自己的 `/mcp`。
- **实现**：进程管理（`tokio::process` 托管 stdio 子进程）+ JSON-RPC 转发 + 工具改名空间（如 `software__chrome__navigate`）。
- **特点**：一次性投入，之后「接入新软件」很大程度变成「接一个现成 MCP」，与路径 A/B/C 互补——**这是本能力域的架构底座之一**，也天然回答了「通用解决方案」诉求。

### 路径选型矩阵

| 路径 | 稳定性 | 开发成本 | 覆盖面 | 风险 | 定位 |
|:---|:---|:---|:---|:---|:---|
| A 原生接口 | 高 | 低 | 中 | 低 | 首选 |
| B 脚本桥 | 中高 | 中 | macOS 原生软件广 | 低 | 次选，通用方案主力 |
| C UI 自动化 | 低 | 高 | 几乎全部 GUI | 中 | 兜底原语 |
| D 注入 Hook | 低 | 高 | 特定软件深 | 高 | 不进主线 |
| E MCP 聚合 | 高 | 中 | 随生态增长 | 低 | 架构底座 |

---

## 三、微信专项分析（首期代表案例）

微信是用户点名场景，也是**最难**的一类目标，先把话说透：

| 途径 | 平台 | 可行性 | 说明 |
|:---|:---|:---|:---|
| 官方 API | 双端 | ❌ | 微信不提供本地/个人号 API；公众号/企业微信是另一套云端体系，不属于「本地软件操作」 |
| AppleScript | macOS | ⚠️ 极有限 | 微信 macOS 未公开脚本字典，只能靠 System Events 做菜单/键盘级操作（本质是路径 C） |
| Accessibility/AX 读屏 | macOS | ⚠️ 可做只读为主 | 需辅助功能授权；可读取会话列表与消息文本、定位窗口、模拟输入发送。界面结构随版本变化，需持续适配 |
| UIA 自动化 | Windows | ✅ 社区方案成熟 | wxauto 等开源项目基于 UIA 实现收发消息；可作为外置进程被 WaLiAPI 通过路径 A（CLI/HTTP 包装）或路径 E（MCP 聚合）接入 |
| 注入 Hook | Windows | ❌ 不进主线 | 见路径 D 结论：封号与合规风险 |

**微信建议落地形态（macOS 环境）**：
1. 首期提供一组**保守工具**：`wechat_list_chats`（读会话列表）、`wechat_read_messages`（读当前会话消息）、`wechat_send_text`（定位窗口 + 输入 + 发送，发送类操作默认需审批）；
2. 实现载体：AX API 读界面树 + enigo 键鼠，封装在「微信连接器」内，对 MCP 只暴露上述语义化工具；
3. 明示限制：仅支持桌面端前台运行、版本升级可能失效、发送操作有误发风险 → 默认开启「发送前人工确认」策略。

**推广到通用原则**：每个「软件连接器」= 路径选型 + 语义化工具集 + 风险分级（只读 / 低危写 / 高危写）+ 审批策略。微信用 C 路径，多数其他软件用 A/B 路径，框架不变。

---

## 四、能力域架构设计提案

### 4.1 模块位置与分层

```
src-tauri/src/services/software/          # 新 Service：SoftwareService（注册进 ServiceRegistry）
├── mod.rs                                # SoftwareService + Service trait impl
├── registry.rs                           # 连接器注册表（对标 channel_presets.rs 模式）
├── executor/                             # 执行底座
│   ├── command.rs                        # 白名单命令执行器（tokio::process，超时/输出截断）
│   ├── script_bridge.rs                  # osascript / powershell 桥
│   ├── ui_automation.rs                  # AX 读屏 + enigo 键鼠（feature gate）
│   └── mcp_proxy.rs                      # 外部 MCP Server 托管与转发（stdio/SSE）
├── connectors/                           # 各软件连接器
│   ├── mod.rs                            # SoftwareConnector trait
│   ├── generic_cli.rs                    # 通用 CLI 连接器（用户自定义命令 → 工具）
│   ├── generic_http.rs                   # 通用本地 HTTP 连接器（后续"接口"能力域复用）
│   ├── wechat.rs                         # 微信（macOS AX 方案）
│   └── ...                               # Finder/Notes/Music/Chrome/Ollama 等
└── routes.rs                             # REST：/api/software/*（管理面）
```

核心抽象（新增代码的最小闭环）：

```rust
#[async_trait]
pub trait SoftwareConnector: Send + Sync {
    fn id(&self) -> &str;                          // "wechat" / "generic_cli"
    fn list_tools(&self, cfg: &SoftwareInstance)   // 动态工具清单（进 tools/list）
        -> Vec<ToolManifest>;
    async fn call_tool(&self, cfg: &SoftwareInstance, name: &str,
        args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult>;
    fn risk_level(&self, name: &str) -> RiskLevel; // ReadOnly / LowWrite / HighWrite
}

pub struct SoftwareInstance {                    // DB 表 software_instances
    id: i64, connector_id: String, name: String,
    config: serde_json::Value,                   // 连接器自定义配置
    mcp_enabled: i64,                            // 对齐 kb/wiki 模式
    approval_mode: String,                       // auto / confirm_high_risk / confirm_all
}
```

### 4.2 MCP 层改造点

1. `get_tools()` 改为**动态组装**：静态 KB/Wiki 工具 + `registry.list_enabled_tools()` 结果；`handle_tool_call` 增加软件工具分发分支（工具名加 `software__` 前缀命名空间，如 `software__wechat__send_text`）。
2. `dispatch_scoped`：软件工具**默认不进入任何 scoped 白名单**；未来如需开放，按实例粒度签发。
3. `McpService::description()` 与 KnowledgeBasePage 的 MCP Tab：工具列表本就从 `mcpService.stats.tools` 渲染，动态化后前端自然跟随；新增「软件对接」Tab 或独立页面。

### 4.3 安全模型（本能力域的重中之重）

| 机制 | 设计 |
|:---|:---|
| 双重开关 | 软件能力全局开关（settings）+ 每实例 `mcp_enabled`，默认全关 |
| 命令白名单 | 通用 CLI 连接器只执行用户显式登记的命令模板，禁止拼接任意参数；禁 shell 直串 |
| 风险分级 + 审批 | `ReadOnly` 自动放行；`HighWrite`（发送消息、删除、支付类）按实例 `approval_mode` 处理；桌面端弹窗确认，headless 端标记 pending 待确认 |
| 审计 | 每次工具调用写 `request_log`（复用现有日志体系），含连接器、工具、参数摘要、结果状态 |
| 扫描器协同 | 本能力域的命令片段送 `security/scanner.rs` 审计，命中规则的发送类操作自动升级为需确认 |
| 鉴权 | 软件工具仅对 MCP Token 全量模式可见；API Key scoped 模式不可见 |
| 桌面权限 | macOS 辅助功能 / 屏幕录制权限在 UI 中引导授予，缺失时连接器状态显示为不可用 |

### 4.4 前端与产物形态

- 新页面「软件对接」（对标 Channels 页模式）：连接器市场（预设注册表）→ 添加实例（表单）→ 启用/停用（`mcp_enabled`）→ 工具列表与审批策略 → 健康状态。
- `tauri-plugin-shell` / 权限声明、macOS entitlements（辅助功能）按需补充；headless 二进制中 UI 自动化路径用 feature gate 排除或降级。
- Web 面板（web/）复用同一页面组件（runtime.ts 适配层照常工作）。

---

## 五、分期路线图

| 阶段 | 内容 | 交付物 | 关键前置 |
|:---|:---|:---|:---|
| **P0 基础设施** | software_instances 表（新迁移文件）、SoftwareService 注册、tools/list 动态化、MCP 管理页扩展 | 框架能跑通一个 hello 级工具 | 无 |
| **P1 通用方案** | 白名单命令执行器 + AppleScript 桥 + 通用 CLI/HTTP 连接器；首批 2-3 个高确定软件（如 Finder/Notes/Music 或用户指定） | 「选软件 → 开启 → MCP 出现工具」端到端 | P0 |
| **P2 微信（macOS）** | AX 读屏 + enigo 键鼠封装 wechat 连接器；审批弹窗机制 | wechat_list_chats / read_messages / send_text | P0、P1；辅助功能权限 |
| **P3 MCP 聚合网关** | 外部 stdio MCP Server 托管、命名空间转发、进程守护 | 接入任一社区 MCP 即获得其工具 | P0 |
| **P4+ 能力域延伸** | 「接口」=  generic_http 深化（OpenAPI 导入 → 工具）；「串口」= serialport crate + 串口连接器，套同一框架 | — | P0/P1 |

> P3 建议尽早：它让「软件对接」从逐个自研变成「自研高价值 + 聚合现成」双轨，边际成本骤降。
> Windows 版微信（UIA 方案）依赖 Windows 环境验证，排期在 P2 之后另行评估。

---

## 六、待决策问题

1. **首期平台范围**：分析按 macOS（当前开发环境）展开，Windows 路径（UIA/PowerShell）是否进入 P1？
2. **审批交互**：桌面端弹窗确认即可，还是 headless 部署也需要完整的待确认队列（涉及 admin API 与前端状态机）？
3. **首批软件清单**：除微信外，优先对接哪些？（建议从路径 A/B 高确定软件选 2-3 个练手）
4. **注入类外挂**：确认不进主线、仅预留连接器接口并文档免责？
5. **UI 自动化依赖**：enigo + macOS AX 引入原生依赖与权限门槛，是否接受（或 P2 改用「截图 + 让 Agent 出坐标」的轻量方案）？

---

## 七、风险清单

| 风险 | 等级 | 缓解 |
|:---|:---|:---|
| 微信界面结构随版本变化导致连接器失效 | 高 | 连接器与版本解耦的定位策略 + 版本适配声明 + 只读优先 |
| 误操作（误发消息/误删数据） | 高 | HighWrite 审批 + 审计 + 默认 confirm 模式 |
| 命令执行被滥用（MCP Token 泄露 = 本机执行权） | 高 | 仅 MCP Token 可见 + 白名单 + 审计 + 建议用户定期轮换 |
| macOS 权限体验差（辅助功能授权门槛） | 中 | UI 引导 + 权限状态检测 |
| 工具数量膨胀导致 Agent 上下文压力 | 中 | mcp_enabled 粒度控制 + 工具按实例聚合命名空间 |
| `security/scanner` 与自身功能互相告警 | 低 | 白名单放行策略同步落地 |
