---
type: ADR
title: 重定义 Tailwind v4 渐变方向变量以剥离 in oklab，兼容旧版 WebKit
description: Tailwind CSS v4 将 bg-gradient-to-* / bg-linear-to-* 编译为 `--tw-gradient-position: to <方向> in oklab`，渐变颜色空间插值语法 `in oklab` 从 WebKit 16.4 才支持，旧版 WKWebView 解析失败会静默丢弃整条 background-image，渐变背景完全丢失（典型症状：仪表盘「网关路由」卡片 WaLiAPI 图标盒蓝色渐变消失、白色图标不可见）；通过在 App.css 中以未分层普通 CSS 重定义同名渐变方向变量（去掉 in oklab）解决。
status: accepted
tags: [frontend, tailwindcss, webkit, compatibility, desktop-ui]
timestamp: 2026-10-16 16:20:00
---

# ADR 0002：重定义 Tailwind v4 渐变方向变量以剥离 in oklab，兼容旧版 WebKit

- 状态：已采纳（accepted）
- 日期：2026-10-16
- 相关代码：`src/App.css`、`src/components/RouteMapCard.tsx`
- 相关决策：[ADR 0001](./0001-tailwind-v4-breakpoint-old-webkit-compat.md)（同一根因家族：Tailwind v4 输出的现代 CSS 语法被旧版 WebKit 静默丢弃）

## 背景（Context）

v0.4.2 发布后的用户反馈：仪表盘新增的「网关路由」动态图中，中间 WaLiAPI 网关节点的图标区域**显示为一片空白**——蓝色渐变图标盒不渲染，盒内的白色 `Radio` 图标（`text-white`）在白底上不可见；同一卡片中的纯色元素（如 `bg-blue-500` 状态圆点）渲染正常。

排查结论：

1. 图标盒使用 `bg-gradient-to-br from-blue-500 to-blue-700`。项目内共有 10+ 个文件、数十处 `bg-gradient-to-*` 用法（LogsPage、UsagePage、ChannelsPage、KnowledgeBasePage 等），均属同一风险面。
2. Tailwind v4.3.3 将 `bg-gradient-to-br` 编译为：

   ```css
   .bg-gradient-to-br {
     --tw-gradient-position: to bottom right in oklab;
     background-image: linear-gradient(var(--tw-gradient-stops));
   }
   ```

3. `--tw-gradient-stops` 展开后 `background-image` 实际为 `linear-gradient(to bottom right in oklab, …)`。渐变颜色空间插值语法 **`in oklab` 从 Safari/WebKit 16.4（2023-03）才开始支持**；旧版 WebKit 解析失败会**静默丢弃整条 `background-image` 声明**，于是渐变背景丢失（ADR 0001 中的 macOS 12.7.5 WKWebView 环境即属此类）。
4. 纯色工具类（如 `bg-blue-500` → `background-color: oklch(…)`）不受影响——`oklch()` 颜色自 Safari 15.4 起支持，这解释了「同一卡片中纯色正常、渐变丢失」的现象。

## 决策（Decision）

在 `src/App.css` 顶部（`@import "tailwindcss";` 与 ADR 0001 的 `@custom-variant` 块之后）以**未分层普通 CSS** 重定义全部 8 个方向的渐变方向变量，去掉 `in oklab`（回退 sRGB 插值），同时覆盖 `bg-gradient-to-*`（v3 旧名，当前代码在用）与 `bg-linear-to-*`（v4 新名，未来演进）：

```css
.bg-gradient-to-t  { --tw-gradient-position: to top; }
.bg-gradient-to-tr { --tw-gradient-position: to top right; }
/* …共 8 个方向，bg-linear-to-* 同样一组… */
```

生效原理：

- Tailwind 的工具类输出在 `@layer utilities` 内；上述规则是**未分层（unlayered）普通 CSS**。按 CSS Cascade 规则，未分层声明优先级高于任何 layer 内的同特异性声明，与书写顺序无关。
- `--tw-gradient-position` 在 Tailwind 内以 `@property { syntax: "*"; inherits: false }` 注册，任意值合法，覆盖安全。
- 覆盖后 `linear-gradient(var(--tw-gradient-stops))` 解析为 `linear-gradient(to bottom right, oklch(…), oklch(…))`：旧 WebKit（15.4+）可完整解析；新浏览器行为不变，仅渐变插值色彩空间从 oklab 回到 sRGB，视觉差异可忽略。

## 备选方案（Alternatives Considered）

| 方案 | 放弃原因 |
| --- | --- |
| 只改 `RouteMapCard.tsx`，把渐变图标盒换成纯色 `bg-blue-600` | 治标不治本：其余 10+ 文件的渐变在旧 WebKit 上仍然全部失效 |
| 组件内用内联 `style={{ backgroundImage: "linear-gradient(...)" }}` 兜底 | 需逐处维护，新增渐变类时必然遗漏 |
| Vite 配置 lightningcss targets 降级转译 | 与 ADR 0001 结论一致：需新增直接依赖、改变全体开发者的 CSS 管线，影响面大 |
| 用 `@supports (background-image: linear-gradient(in lab, red, red))` 包裹降级 | Tailwind 对 radial 已采用此手法，但 @supports 查询本身在旧 WebKit 中行为一致可用；不过手写 @supports 包裹每条 Tailwind 原规则不可行（无法拦截 JIT 生成的规则），最终还是落到对同名类重定义变量，等价于本方案 |

## 影响（Consequences）

正面：

- 零新增依赖、零组件改动，修复对开发模式与正式构建（桌面安装包 / Web 面板）同时生效；全代码库 10+ 文件的渐变在旧版 WebKit 上一次性恢复。
- 新版浏览器行为不变（两种写法在支持的引擎中均合法），仅插值色彩空间差异，视觉几乎无感。

注意事项：

- 若未来 Tailwind 更改渐变变量的实现方式（如不再经 `--tw-gradient-position` 组合 stops），需同步此处；自 v4 发布以来该结构稳定，风险低。
- 渐变的**角度语法**（`bg-linear-45` 等）与 `bg-radial-*`、`bg-conic-*` 若未来引入，需评估其编译产物是否同样携带 `in oklab` 并补充对应覆盖。

## 验证（Verification）

1. `pnpm build` 后检查 `dist/assets/index-*.css`：`.bg-gradient-to-br` 同时存在 `to bottom right in oklab`（Tailwind 原规则，@layer 内，被覆盖失效）与 `to bottom right`（未分层覆盖，实际生效）两条规则。
2. 构建通过 `tsc` 类型检查（`pnpm build`），无新增告警。
