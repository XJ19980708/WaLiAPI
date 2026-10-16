import { useEffect, useState, useMemo, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { statsApi } from "../lib/api";
import type { DashboardStats, ModelStats, TokenTrendPoint, PromptPartStat } from "../types";
import { RouteMapCard } from "../components/RouteMapCard";
import { TokenHeatmap } from "../components/TokenHeatmap";
import { formatNumber, formatDuration } from "../lib/constants";
import {
  Activity,
  Radio,
  Key,
  Zap,
  TrendingUp,
  ShieldCheck,
  Workflow,
  Plus,
  BookOpen,
  FileText,
  Globe,
  HelpCircle,
  X,
  Check,
  Database,
  Layers,
  Terminal,
  Network,
  Puzzle,
  DatabaseZap,
  ChevronDown,
  ChevronUp,
} from "lucide-react";

export function DashboardPage() {
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [modelStats, setModelStats] = useState<ModelStats[]>([]);
  const [tokenTrend, setTokenTrend] = useState<TokenTrendPoint[]>([]);
  const [partStats, setPartStats] = useState<PromptPartStat[]>([]);
  const [trendHours, setTrendHours] = useState<24 | 168 | 720>(24);
  const [showInputToken, setShowInputToken] = useState(true);
  const [showOutputToken, setShowOutputToken] = useState(true);
  const [showCachedToken, setShowCachedToken] = useState(true);
  const [hoverBar, setHoverBar] = useState<{ x: number; y: number; hour: string; data: { model: string; input: number; output: number; cached: number }[] } | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [showHelp, setShowHelp] = useState(false);
  const navigate = useNavigate();

  useEffect(() => {
    const doLoad = () => statsApi.getDashboard().then(setStats).catch(() => setLoadError(true));
    doLoad();
    const interval = setInterval(doLoad, 10000);
    return () => clearInterval(interval);
  }, []);

  useEffect(() => {
    const doLoad = () => statsApi.getModelStats().then(setModelStats).catch(() => {});
    doLoad();
    const interval = setInterval(doLoad, 10000);
    return () => clearInterval(interval);
  }, []);

  useEffect(() => {
    const doLoad = () => statsApi.getTokenTrend(trendHours).then(setTokenTrend).catch(() => {});
    doLoad();
    const interval = setInterval(doLoad, 30000);
    return () => clearInterval(interval);
  }, [trendHours]);

  // 上下文构成（迁移 048）：近 24h 按消息分类的 token 聚合，与趋势图同节奏刷新。
  useEffect(() => {
    const doLoad = () => statsApi.getPromptPartStats(24).then(setPartStats).catch(() => {});
    doLoad();
    const interval = setInterval(doLoad, 30000);
    return () => clearInterval(interval);
  }, []);

  if (loadError && !stats) {
    return (
      <div className="page-shell flex flex-col items-center justify-center gap-3 text-sm text-slate-500">
        <p>数据加载失败，请检查服务是否已启动。</p>
        <button onClick={() => window.location.reload()} className="rounded-lg bg-blue-600 px-4 py-2 text-xs font-medium text-white hover:bg-blue-700">重新加载</button>
      </div>
    );
  }

  if (!stats) {
    return <div className="page-shell text-sm text-slate-500">加载中...</div>;
  }

  // 服务可用率 = 可用上游 / 全部上游，上游包含 API 渠道与 Auth 账号两类。
  // 字段缺失（旧后端 + 新前端的版本错位）按 0 优雅降级，避免 NaN（NEW-4）。
  const activeUpstreams = (stats.active_channels ?? 0) + (stats.active_auth_accounts ?? 0);
  const totalUpstreams = (stats.total_channels ?? 0) + (stats.total_auth_accounts ?? 0);
  const availability = totalUpstreams > 0 ? Math.round((activeUpstreams / totalUpstreams) * 100) : 0;

  // 上 5：请求与渠道 | 下 5：知识服务
  const cacheRate = (cached: number, prompt: number) =>
    prompt > 0 ? `${Math.round((cached / prompt) * 100)}%` : "0%";

  const topMetrics = [
    { label: "今日请求", value: formatNumber(stats.today_requests), icon: Activity, color: "text-blue-600", tone: "bg-blue-50" },
    { label: "今日 Token", value: formatNumber(stats.today_total_tokens), icon: Zap, color: "text-amber-600", tone: "bg-amber-50" },
    { label: "今日缓存命中", value: cacheRate(stats.today_cached_tokens, stats.today_prompt_tokens), sub: `节省 ${formatNumber(stats.today_cached_tokens)} tokens`, icon: DatabaseZap, color: "text-emerald-600", tone: "bg-emerald-50" },
    { label: "平均延迟", value: formatDuration(Math.round(stats.avg_latency_ms)), sub: stats.avg_ttft_ms > 0 ? `首字 ${formatDuration(Math.round(stats.avg_ttft_ms))}` : undefined, icon: Workflow, color: "text-violet-600", tone: "bg-violet-50" },
    { label: "活跃上游", value: `${activeUpstreams}/${totalUpstreams}`, sub: `渠道 ${stats.active_channels}/${stats.total_channels} · 账号 ${stats.active_auth_accounts}/${stats.total_auth_accounts}`, icon: Radio, color: "text-teal-600", tone: "bg-teal-50" },
    { label: "累计 Token", value: formatNumber(stats.total_tokens), sub: `${formatNumber(stats.total_requests)} 次请求`, icon: TrendingUp, color: "text-indigo-600", tone: "bg-indigo-50" },
  ];

  const quickActions = [
    { title: "新建渠道", icon: Plus, action: () => navigate("/channels") },
    { title: "管理密钥", icon: Key, action: () => navigate("/api-keys") },
    { title: "创建 RAG", icon: Database, action: () => navigate("/services/knowledge-base") },
    { title: "Wiki 知识库", icon: Network, action: () => navigate("/services/wiki") },
    { title: "Skills", icon: Puzzle, action: () => navigate("/services/skills") },
    { title: "接入示例", icon: BookOpen, action: () => navigate("/usage") },
    { title: "审计日志", icon: FileText, action: () => navigate("/logs") },
    { title: "安全设置", icon: ShieldCheck, action: () => navigate("/settings") },
    { title: "渠道管理", icon: Globe, action: () => navigate("/channels") },
    { title: "MCP", icon: Terminal, action: () => navigate("/services/mcp") },
  ];

  return (
    <div className="page-shell space-y-5">
      {/* 顶部：欢迎 + 快速操作 */}
      <section className="surface rounded-[24px] p-6 md:p-7">
        <div className="flex flex-col gap-5 xl:flex-row xl:items-start xl:justify-between">
          <div className="max-w-2xl">
            <div className="inline-flex items-center gap-2 rounded-full border border-blue-100 bg-blue-50 px-3 py-1 text-xs font-medium text-blue-700">
              <Workflow className="h-3.5 w-3.5" /> 控制台首页
            </div>
            <div className="mt-4 flex items-center gap-2">
              <h1 className="text-3xl font-semibold tracking-[-0.03em] text-slate-900">欢迎使用 WaLiAPI</h1>
              <button
                onClick={() => setShowHelp(true)}
                className="inline-flex items-center justify-center rounded-full border border-slate-200 bg-slate-50 p-1 text-slate-400 transition-all hover:border-blue-200 hover:bg-blue-50 hover:text-blue-600"
                title="使用帮助"
              >
                <HelpCircle className="h-4 w-4" />
              </button>
            </div>
            <p className="mt-2.5 text-sm leading-6 text-slate-500 md:text-[15px]">
              在一个统一入口中管理上游模型渠道、下游密钥、请求统计与故障切换，让本地 LLM 网关更稳定、更清晰、更易运维。
            </p>

            {/* 快速操作按钮 */}
            <div className="mt-5 flex flex-wrap gap-2">
              {quickActions.map(({ title, icon: Icon, action }) => (
                <button
                  key={title}
                  onClick={action}
                  className="inline-flex items-center gap-1.5 rounded-full border border-slate-200 bg-slate-50 px-3 py-1.5 text-xs font-medium text-slate-700 transition-all hover:border-blue-200 hover:bg-white hover:text-blue-700 hover:shadow-sm"
                >
                  <Icon className="h-3.5 w-3.5" />
                  {title}
                </button>
              ))}
            </div>
          </div>

          {/* 健康度徽章 */}
          <div className="flex gap-3 xl:w-auto">
            <div className={`flex items-center gap-2.5 rounded-2xl border px-4 py-3 ${availability >= 80 ? "border-emerald-200 bg-emerald-50" : availability >= 50 ? "border-amber-200 bg-amber-50" : "border-rose-200 bg-rose-50"}`}>
              <ShieldCheck className={`h-5 w-5 ${availability >= 80 ? "text-emerald-600" : availability >= 50 ? "text-amber-600" : "text-rose-600"}`} />
              <div>
                <div className="text-xs text-slate-500">服务可用率</div>
                <div className={`text-lg font-semibold ${availability >= 80 ? "text-emerald-700" : availability >= 50 ? "text-amber-700" : "text-rose-700"}`}>{availability}%</div>
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* 指标卡片 — 单排核心指标（延迟/首字/上游/累计合并进来，知识服务降级为尾部小卡） */}
      <div className="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-6">
        {topMetrics.map(({ label, value, sub, icon: Icon, color, tone }) => (
          <div key={label} className="surface data-card">
            <div className="flex items-center justify-between">
              <div className={`rounded-xl ${tone} p-2`}>
                <Icon className={`h-4 w-4 ${color}`} />
              </div>
            </div>
            <div className="mt-3 text-2xl font-semibold tracking-tight text-slate-900">{value}</div>
            <div className="text-xs text-slate-500">{label}</div>
            {sub && <div className="mt-0.5 text-[11px] text-slate-400">{sub}</div>}
          </div>
        ))}
      </div>

      {/* 知识服务一行小卡（从指标区降级，弱化视觉重量） */}
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2 rounded-2xl border border-slate-200 bg-white/60 px-5 py-3 text-xs text-slate-500">
        <span className="flex items-center gap-1.5 font-medium text-slate-600">
          <Database className="h-3.5 w-3.5 text-cyan-500" /> 知识服务
        </span>
        <span>RAG <b className="font-semibold text-slate-700">{stats.total_knowledge_bases}</b> 库 · {stats.total_kb_documents} 文档 · {stats.total_kb_chunks} 切片</span>
        <span>Wiki <b className="font-semibold text-slate-700">{stats.total_wiki_projects}</b> 项目 · {stats.total_wiki_pages} 页面</span>
        <span>缓存命中 <b className="font-semibold text-slate-700">{cacheRate(stats.total_cached_tokens, stats.total_prompt_tokens)}</b>（累计节省 {formatNumber(stats.total_cached_tokens)} tokens）</span>
      </div>

      {/* 网关路由动态图（调用方 → 网关 → 渠道，可观测性 P4） */}
      <RouteMapCard />

      {/* Token 趋势（左）+ Token 热力图（右）：宽屏左右分栏（1:1），窄屏纵向堆叠 */}
      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <div className="min-w-0">
          <TokenTrendChart
            data={tokenTrend}
            hours={trendHours}
            showInput={showInputToken}
            showOutput={showOutputToken}
            showCached={showCachedToken}
            onHoursChange={setTrendHours}
            onToggleInput={() => setShowInputToken(v => !v)}
            onToggleOutput={() => setShowOutputToken(v => !v)}
            onToggleCached={() => setShowCachedToken(v => !v)}
            hoverBar={hoverBar}
            setHoverBar={setHoverBar}
          />
        </div>
        <div className="min-w-0">
          <TokenHeatmap />
        </div>
      </div>

      {/* 模型分布（通栏紧凑列表） */}
      <ModelDistributionTable data={modelStats} />

      {/* 上下文构成（近 24h 按消息分类的 token 聚合，紧凑单行） */}
      <PromptPartComposition data={partStats} />

      {/* 运维建议 */}
      <section className="surface rounded-[20px] p-6">
        <div className="flex items-center justify-between">
          <div>
            <h2 className="text-lg font-semibold text-slate-900">运维建议</h2>
            <p className="mt-1 text-sm text-slate-500">根据当前系统状态给出的运维参考</p>
          </div>
          <TrendingUp className="h-5 w-5 text-slate-400" />
        </div>
        <div className="mt-5 grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
          <div className="rounded-2xl border border-slate-200 bg-slate-50 p-4">
            <div className="flex items-center gap-2">
              <Radio className="h-4 w-4 text-emerald-600" />
              <span className="text-sm font-medium text-slate-900">渠道健康度</span>
            </div>
            <p className="mt-1.5 text-sm text-slate-500">
              {availability >= 80
                ? "当前上游运行正常，渠道与账号线路可用。"
                : availability >= 50
                  ? "部分上游不可用，建议检查渠道与 Auth 账号并启用备用线路。"
                  : "活跃上游较少，请前往渠道/Auth 账号页测试并启用。"}
            </p>
          </div>
          <div className="rounded-2xl border border-slate-200 bg-slate-50 p-4">
            <div className="flex items-center gap-2">
              <Key className="h-4 w-4 text-indigo-600" />
              <span className="text-sm font-medium text-slate-900">密钥配额</span>
            </div>
            <p className="mt-1.5 text-sm text-slate-500">
              {stats.total_api_keys > 0
                ? `共 ${stats.total_api_keys} 个密钥，定期检查配额使用情况。`
                : "尚未创建密钥，请前往 API 密钥页创建。"}
            </p>
          </div>
          <div className="rounded-2xl border border-slate-200 bg-slate-50 p-4">
            <div className="flex items-center gap-2">
              <Activity className="h-4 w-4 text-blue-600" />
              <span className="text-sm font-medium text-slate-900">性能监控</span>
            </div>
            <p className="mt-1.5 text-sm text-slate-500">
              {stats.avg_latency_ms < 2000
                ? `平均延迟 ${formatDuration(Math.round(stats.avg_latency_ms))}，响应正常。`
                : `平均延迟 ${formatDuration(Math.round(stats.avg_latency_ms))}，建议查看日志排查慢请求。`}
            </p>
          </div>
          <div className="rounded-2xl border border-slate-200 bg-slate-50 p-4">
            <div className="flex items-center gap-2">
              <Network className="h-4 w-4 text-fuchsia-600" />
              <span className="text-sm font-medium text-slate-900">知识服务</span>
            </div>
            <p className="mt-1.5 text-sm text-slate-500">
              {stats.total_knowledge_bases > 0 || stats.total_wiki_projects > 0
                ? `${stats.total_knowledge_bases} 个 RAG · ${stats.total_wiki_projects} 个 Wiki，向量与投影正常运行。`
                : "尚未创建 RAG / Wiki，点击上方「创建 RAG」开始。"}
            </p>
          </div>
        </div>
      </section>

      {/* 使用帮助弹窗 */}
      {showHelp && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center overflow-y-auto bg-black/40 p-4 backdrop-blur-sm"
          onClick={() => setShowHelp(false)}
        >
          <div
            className="relative my-auto w-full max-w-lg max-h-[85vh] overflow-y-auto rounded-3xl bg-white p-7 shadow-2xl"
            onClick={e => e.stopPropagation()}
          >
            <button
              onClick={() => setShowHelp(false)}
              className="absolute right-5 top-5 rounded-full p-1 text-slate-400 transition-colors hover:bg-slate-100 hover:text-slate-600"
            >
              <X className="h-5 w-5" />
            </button>

            <div className="flex items-center gap-2">
              <div className="rounded-2xl border border-blue-100 bg-blue-50 p-2.5">
                <HelpCircle className="h-5 w-5 text-blue-600" />
              </div>
              <div>
                <h2 className="text-lg font-semibold text-slate-900">快速上手指南</h2>
                <p className="text-xs text-slate-500">几步完成本地 LLM 网关配置</p>
              </div>
            </div>

            <div className="mt-5 space-y-3.5">
              {[
            {
              num: "1",
              required: true,
              title: "添加上游渠道",
              desc: "进入「渠道管理」页面，点击「新建渠道」，填写名称、Base URL、API Key 和支持的模型，保存即可。",
              route: "/channels",
              routeLabel: "前往渠道管理",
            },
            {
              num: "2",
              required: true,
              title: "创建本地密钥",
              desc: "进入「API 密钥」页面，点击「新建密钥」生成 `sk-waliapi-*` 格式的本地访问令牌，用于下游客户端调用。",
              route: "/api-keys",
              routeLabel: "前往 API 密钥",
            },
            {
              num: "3",
              required: true,
              title: "查看接入示例",
              desc: "进入「接入示例」页面，复制 cURL / Python / Node.js 代码，将 `base_url` 指向 `http://127.0.0.1:8777/v1`，使用本地密钥即可调用。",
              route: "/usage",
              routeLabel: "前往接入示例",
            },
            {
              num: "4",
              required: false,
              title: "配置服务与重试",
              desc: "在「设置 → 服务配置」中调整监听地址与端口；在「重试策略」中开启失败自动重试，提升服务稳定性。",
              route: "/settings",
              routeLabel: "前往设置",
            },
            {
              num: "5",
              required: false,
              title: "开启安全审计",
              desc: "在「设置 → 安全审计」中启用请求风险检测，自动识别凭证泄露、敏感路径、工具外联与 Unicode 隐写。",
              route: "/settings",
              routeLabel: "前往安全设置",
            },
          ].map(step => (
                <div
                  key={step.num}
                  className="rounded-2xl border border-slate-200 bg-slate-50 p-4"
                >
                  <div className="flex items-start gap-3">
                    <div className={`flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold text-white ${step.required ? "bg-blue-600" : "bg-slate-400"}`}>
                      {step.num}
                    </div>
                    <div className="flex-1">
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-medium text-slate-900">{step.title}</span>
                        {step.required ? (
                          <span className="inline-flex items-center gap-0.5 rounded-full bg-blue-100 px-1.5 py-0.5 text-[10px] font-medium text-blue-700">
                            <Check className="h-2.5 w-2.5" />必选
                          </span>
                        ) : (
                          <span className="inline-flex items-center rounded-full bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium text-slate-500">
                            可选
                          </span>
                        )}
                      </div>
                      <p className="mt-1 text-sm leading-5 text-slate-500">{step.desc}</p>
                      <button
                        onClick={() => {
                          navigate(step.route);
                          setShowHelp(false);
                        }}
                        className="mt-2 inline-flex items-center gap-1 text-xs font-medium text-blue-600 hover:text-blue-700"
                      >
                        {step.routeLabel}
                        <span aria-hidden>→</span>
                      </button>
                    </div>
                  </div>
                </div>
              ))}
            </div>

            <div className="mt-5 rounded-2xl border border-emerald-100 bg-emerald-50 p-4">
              <div className="flex items-start gap-2.5">
                <FileText className="mt-0.5 h-4 w-4 shrink-0 text-emerald-600" />
                <div>
                  <div className="text-sm font-medium text-emerald-900">调用后可查看审计日志</div>
                  <p className="mt-1 text-xs leading-5 text-emerald-700">
                    发起请求后，进入「审计日志」页面查看每次调用的状态码、Token 消耗、工具调用、安全风险等级与上游路由详情。
                  </p>
                  <button
                    onClick={() => {
                      navigate("/logs");
                      setShowHelp(false);
                    }}
                    className="mt-2 inline-flex items-center gap-1 text-xs font-medium text-emerald-700 hover:text-emerald-800"
                  >
                    前往审计日志<span aria-hidden>→</span>
                  </button>
                </div>
              </div>
            </div>

            <div className="mt-4 flex items-center justify-between rounded-2xl bg-slate-100 px-4 py-3">
              <span className="text-xs text-slate-500">
                <span className="font-semibold text-slate-700">1、2、3</span> 为必选步骤 ·{" "}
                <span className="font-semibold text-slate-700">4、5</span> 为可选增强
              </span>
              <button
                onClick={() => setShowHelp(false)}
                className="rounded-full bg-slate-900 px-4 py-1.5 text-xs font-medium text-white transition-colors hover:bg-slate-700"
              >
                我知道了
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ════════════════════════════════════════════════════════════
// 上下文构成（近 24h 按消息分类的 token 聚合，迁移 048）
// ════════════════════════════════════════════════════════════

// 与 src/components/logs/ObservabilityCards.tsx 的 PROMPT_PART_META 保持一致。
const PART_META: Record<string, { label: string; bar: string; text: string }> = {
  system:      { label: "系统提示词", bar: "bg-slate-400",   text: "text-slate-600" },
  tools:       { label: "工具定义",   bar: "bg-purple-400", text: "text-purple-600" },
  injected:    { label: "注入内容",   bar: "bg-amber-400",  text: "text-amber-600" },
  assistant:   { label: "助手消息",   bar: "bg-emerald-400", text: "text-emerald-600" },
  tool_result: { label: "工具结果",   bar: "bg-blue-400",   text: "text-blue-600" },
  chat:        { label: "用户消息",   bar: "bg-sky-400",    text: "text-sky-600" },
};

function PromptPartComposition({ data }: { data: PromptPartStat[] }) {
  const nonZero = data.filter(d => d.tokens > 0);
  if (nonZero.length === 0) return null;
  const total = nonZero.reduce((sum, d) => sum + d.tokens, 0);
  return (
    <section className="surface rounded-[24px] p-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-sm font-semibold text-slate-900">上下文构成</h2>
        <span className="text-xs text-slate-400">近 24 小时 · 共 {formatNumber(total)} tokens</span>
      </div>
      {/* 单行紧凑布局：比例条与图例同行，减少纵向空间 */}
      <div className="mt-3 flex flex-wrap items-center gap-x-5 gap-y-2">
        <div className="flex h-2.5 min-w-[120px] flex-1 overflow-hidden rounded-full bg-slate-100">
          {nonZero.map(d => {
            const meta = PART_META[d.kind] || { label: d.kind, bar: "bg-slate-300", text: "text-slate-500" };
            return (
              <div
                key={d.kind}
                className={meta.bar}
                style={{ width: `${(d.tokens / total) * 100}%` }}
                title={`${meta.label}: ${formatNumber(d.tokens)} (${Math.round((d.tokens / total) * 100)}%)`}
              />
            );
          })}
        </div>
        <div className="flex flex-wrap gap-x-4 gap-y-1.5">
          {nonZero.map(d => {
            const meta = PART_META[d.kind] || { label: d.kind, bar: "bg-slate-300", text: "text-slate-500" };
            return (
              <div key={d.kind} className="flex items-center gap-1.5 text-[11px]">
                <span className={`h-2 w-2 rounded-sm ${meta.bar}`} />
                <span className={`font-medium ${meta.text}`}>{meta.label}</span>
                <span className="tabular-nums text-slate-500">{formatNumber(d.tokens)}</span>
                <span className="tabular-nums text-slate-400">{Math.round((d.tokens / total) * 100)}%</span>
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

// ════════════════════════════════════════════════════════════
// 模型分布表格
// ════════════════════════════════════════════════════════════

const MODEL_COLORS = [
  "bg-blue-500", "bg-emerald-500", "bg-violet-500", "bg-amber-500",
  "bg-rose-500", "bg-cyan-500", "bg-indigo-500", "bg-teal-500",
  "bg-fuchsia-500", "bg-orange-500",
];

function ModelDistributionTable({ data }: { data: ModelStats[] }) {
  const maxTokens = Math.max(...data.map(d => d.total_tokens), 1);
  // 默认展示 8 个模型,更多折叠;模型少时无需展开。
  const COLLAPSED_ROWS = 8;
  const [expanded, setExpanded] = useState(false);
  const visible = expanded ? data : data.slice(0, COLLAPSED_ROWS);
  const canExpand = data.length > COLLAPSED_ROWS;

  return (
    <section className="surface h-full rounded-[20px] p-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-lg font-semibold text-slate-900">模型分布</h2>
          <p className="mt-1 text-sm text-slate-500">Token 消耗与成功率（按模型）</p>
        </div>
        <Layers className="h-5 w-5 text-slate-400" />
      </div>

      {data.length === 0 ? (
        <div className="mt-5 rounded-2xl border border-slate-200 bg-slate-50 p-8 text-center text-sm text-slate-400">
          暂无调用记录
        </div>
      ) : (
        <div className="mt-5 space-y-3.5">
          {visible.map((row, i) => {
            const share = maxTokens > 0 ? (row.total_tokens / maxTokens) * 100 : 0;
            const cached = row.cached_tokens ?? 0;
            const successRate = row.success_rate * 100;
            return (
              <div key={row.model}>
                <div className="flex items-center gap-2 text-sm">
                  <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${MODEL_COLORS[i % MODEL_COLORS.length]}`} />
                  <span className="min-w-0 flex-1 truncate font-medium text-slate-900" title={row.model}>{row.model}</span>
                  <span className="shrink-0 text-xs tabular-nums text-slate-400">{formatNumber(row.request_count)} 次</span>
                  <span className="w-20 shrink-0 text-right font-semibold tabular-nums text-slate-800">
                    {formatNumber(row.total_tokens)}
                  </span>
                </div>
                {/* token 占比条 + 缓存命中 + 成功率（同行紧凑排） */}
                <div className="mt-1.5 ml-4 flex items-center gap-2.5">
                  <div className="h-1.5 min-w-[40px] flex-1 overflow-hidden rounded-full bg-slate-100">
                    <div
                      className={`h-full rounded-full ${MODEL_COLORS[i % MODEL_COLORS.length]}`}
                      style={{ width: `${share}%` }}
                    />
                  </div>
                  <span className="w-9 shrink-0 text-right text-[10px] tabular-nums text-slate-400">
                    {Math.round(share)}%
                  </span>
                  {cached > 0 && (
                    <span className="shrink-0 text-[10px] tabular-nums text-emerald-600" title="缓存命中（占输入）">
                      ⚡{row.input_tokens > 0 ? `${((cached / row.input_tokens) * 100).toFixed(0)}%` : formatNumber(cached)}
                    </span>
                  )}
                  <span
                    className={`w-12 shrink-0 text-right text-[11px] tabular-nums ${
                      successRate >= 95 ? "text-emerald-600" : successRate >= 80 ? "text-amber-600" : "text-rose-600"
                    }`}
                    title="成功率"
                  >
                    {successRate.toFixed(1)}%
                  </span>
                </div>
              </div>
            );
          })}

          {/* 展开/收起按钮:模块底部居中,淡色边框 hover 强调色(主题审美) */}
          {canExpand && (
            <div className="flex justify-center pt-1">
              <button
                onClick={() => setExpanded(e => !e)}
                className="flex items-center gap-1.5 rounded-lg border border-slate-200 px-4 py-1.5 text-xs font-medium text-slate-500 transition-all hover:border-blue-400 hover:text-blue-600"
              >
                {expanded ? (
                  <>
                    <ChevronUp size={13} />
                    收起
                  </>
                ) : (
                  <>
                    <ChevronDown size={13} />
                    展开更多（{data.length - COLLAPSED_ROWS}）
                  </>
                )}
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

// ════════════════════════════════════════════════════════════
// Token 使用趋势图 (纯 SVG 堆叠柱状图)
// ════════════════════════════════════════════════════════════

const TREND_LINE_COLORS = [
  { stroke: "#3b82f6", fill: "#3b82f6", light: "#93c5fd" }, // blue
  { stroke: "#10b981", fill: "#10b981", light: "#6ee7b7" }, // emerald
  { stroke: "#8b5cf6", fill: "#8b5cf6", light: "#c4b5fd" }, // violet
  { stroke: "#f59e0b", fill: "#f59e0b", light: "#fcd34d" }, // amber
  { stroke: "#ef4444", fill: "#ef4444", light: "#fca5a5" }, // rose
  { stroke: "#06b6d4", fill: "#06b6d4", light: "#67e8f9" }, // cyan
  { stroke: "#6366f1", fill: "#6366f1", light: "#a5b4fc" }, // indigo
  { stroke: "#14b8a6", fill: "#14b8a6", light: "#5eead4" }, // teal
  { stroke: "#d946ef", fill: "#d946ef", light: "#f0abfc" }, // fuchsia
  { stroke: "#f97316", fill: "#f97316", light: "#fdba74" }, // orange
];

interface TrendHover {
  x: number;
  y: number;
  hour: string;
  data: { model: string; input: number; output: number; cached: number }[];
}

function TokenTrendChart({
  data,
  hours,
  showInput,
  showOutput,
  showCached,
  onHoursChange,
  onToggleInput,
  onToggleOutput,
  onToggleCached,
  hoverBar,
  setHoverBar,
}: {
  data: TokenTrendPoint[];
  hours: 24 | 168 | 720;
  showInput: boolean;
  showOutput: boolean;
  showCached: boolean;
  onHoursChange: (h: 24 | 168 | 720) => void;
  onToggleInput: () => void;
  onToggleOutput: () => void;
  onToggleCached: () => void;
  hoverBar: TrendHover | null;
  setHoverBar: (h: TrendHover | null) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerW, setContainerW] = useState(800);

  useEffect(() => {
    const update = () => setContainerW(containerRef.current?.clientWidth ?? 800);
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);

  // 按小时聚合数据
  const { hoursList, modelList, seriesByModel } = useMemo(() => {
    const modelSet = new Set<string>();
    data.forEach(d => modelSet.add(d.model));
    const modelsArr = Array.from(modelSet);

    // 构建原始数据 map: hour -> model -> point
    const map = new Map<string, Map<string, TokenTrendPoint>>();
    data.forEach(d => {
      if (!map.has(d.hour)) map.set(d.hour, new Map());
      map.get(d.hour)!.set(d.model, d);
    });

    // 根据查询范围决定聚合粒度
    // 日(24h): 1 小时一个点 → 24 点
    // 周(168h): 3 小时一个点 → 56 点
    // 月(720h): 1 天一个点 → 30 点
    const bucketMs = hours === 24 ? 3600_000 : hours === 168 ? 3 * 3600_000 : 86_400_000;
    const totalBuckets = hours === 24 ? 24 : hours === 168 ? 56 : 30;

    const now = new Date();
    // 对齐到 bucket 起点
    const start = new Date(now);
    if (bucketMs >= 86_400_000) {
      start.setHours(0, 0, 0, 0);
    } else {
      start.setMinutes(0, 0, 0);
      start.setMinutes(start.getMinutes() - (start.getMinutes() % (bucketMs / 60_000)));
    }
    start.setTime(start.getTime() - bucketMs * (totalBuckets - 1));

    // 生成 bucket 标签和聚合数据
    const hoursArr: string[] = [];
    for (let i = 0; i < totalBuckets; i++) {
      const d = new Date(start.getTime() + i * bucketMs);
      hoursArr.push(d.toISOString());
    }

    // 将原始数据按 bucket 聚合
    const series = modelsArr.map(m => ({
      model: m,
      points: hoursArr.map(h => {
        const bucketStart = new Date(h);
        const bucketEnd = new Date(bucketStart.getTime() + bucketMs);
        // 在此 bucket 范围内累加该模型的数据
        let input = 0, output = 0, cached = 0, total = 0, requests = 0;
        for (const [rawHour, modelMap] of map) {
          const rawDate = new Date(rawHour);
          if (rawDate >= bucketStart && rawDate < bucketEnd) {
            const p = modelMap.get(m);
            if (p) {
              input += p.input_tokens;
              output += p.output_tokens;
              cached += p.cached_tokens ?? 0;
              total += p.total_tokens;
              requests += p.request_count;
            }
          }
        }
        return { hour: h, input, output, cached, total, requests };
      }),
    }));

    return { hoursList: hoursArr, modelList: modelsArr, seriesByModel: series };
  }, [data, hours]);

  // 图例点击显隐模型（隐藏后曲线/hover/Y轴都不再计入）
  const [hiddenModels, setHiddenModels] = useState<Set<string>>(new Set());
  const toggleModel = (m: string) => {
    setHiddenModels(prev => {
      const next = new Set(prev);
      if (next.has(m)) next.delete(m); else next.add(m);
      return next;
    });
  };
  const visibleSeries = useMemo(
    () => seriesByModel.filter(s => !hiddenModels.has(s.model)),
    [seriesByModel, hiddenModels]
  );

  const chartH = 220;
  const padding = { top: 20, right: 16, bottom: 36, left: 52 };
  const chartW = Math.max(containerW - padding.left - padding.right, 100);
  const stepX = hoursList.length > 1 ? chartW / (hoursList.length - 1) : chartW;

  // 每桶的堆叠柱数据:柱高 = 该桶内各可见模型「选中类型求和」的累计;
  // 隐藏的模型不参与堆叠,柱高随之下降。hover 浮层读取的是同一份 visibleSeries。
  const stackedBars = useMemo(() => {
    return hoursList.map((h, i) => {
      const segments: { model: string; value: number; colorIdx: number }[] = [];
      visibleSeries.forEach(s => {
        const p = s.points[i];
        if (!p) return;
        let value = 0;
        if (showInput) value += p.input;
        if (showOutput) value += p.output;
        if (showCached) value += p.cached;
        if (value > 0) {
          segments.push({ model: s.model, value, colorIdx: Math.max(0, modelList.indexOf(s.model)) });
        }
      });
      const total = segments.reduce((a, s) => a + s.value, 0);
      return { hour: h, total, segments };
    });
  }, [visibleSeries, hoursList, modelList, showInput, showOutput, showCached]);

  // Y 轴最大值:各桶堆叠总和的峰值,留 15% headroom。
  const maxValue = useMemo(() => {
    let max = 0;
    stackedBars.forEach(b => {
      if (b.total > max) max = b.total;
    });
    return max > 0 ? max * 1.15 : 1;
  }, [stackedBars]);

  const yTicks = 5;
  const yTickValues = Array.from({ length: yTicks + 1 }, (_, i) => (maxValue / yTicks) * i);

  const formatHourLabel = (h: string) => {
    const d = new Date(h);
    if (hours === 24) return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, "0")}:00`;
    if (hours === 168) return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, "0")}:00`;
    return `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`;
  };

  const formatHourShort = (h: string) => {
    const d = new Date(h);
    if (hours === 24) return `${String(d.getHours()).padStart(2, "0")}:00`;
    if (hours === 168) return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, "0")}h`;
    return `${d.getMonth() + 1}/${d.getDate()}`;
  };

  const formatTick = (v: number) => {
    if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
    if (v >= 1_000) return `${(v / 1_000).toFixed(1)}K`;
    return String(Math.round(v));
  };

  const labelInterval = Math.max(1, Math.floor(hoursList.length / 10));

  // hover 十字线 x 坐标 → 最近的数据点索引
  const hoverIndex = hoverBar
    ? Math.round((hoverBar.x - padding.left) / stepX)
    : -1;
  const clampedHoverIndex = Math.max(0, Math.min(hoverIndex, hoursList.length - 1));

  return (
    <section className="surface rounded-[20px] p-6">
      {/* 头部：标题 + 日/周/月切换 */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h2 className="text-lg font-semibold text-slate-900">Token 使用趋势</h2>
          <p className="mt-1 text-sm text-slate-500">
            {hours === 24 ? "最近 24 小时" : hours === 168 ? "最近 7 天" : "最近 30 天"} · {hours === 24 ? "按小时" : hours === 168 ? "按 3 小时" : "按天"}粒度 · 按模型颜色堆叠
          </p>
        </div>
        {/* 日/周/月切换 */}
        <div className="flex items-center gap-0.5 rounded-full border border-slate-200 bg-white p-0.5">
          {([
            { v: 24, label: "日" },
            { v: 168, label: "周" },
            { v: 720, label: "月" },
          ] as const).map(opt => (
            <button
              key={opt.v}
              onClick={() => onHoursChange(opt.v)}
              className={`whitespace-nowrap rounded-full px-3.5 py-1 text-xs font-medium transition-all ${
                hours === opt.v ? "bg-slate-900 text-white shadow-sm" : "text-slate-500 hover:text-slate-700"
              }`}
            >
              {opt.label}
            </button>
          ))}
        </div>
      </div>

      {/* 图例 */}
      {modelList.length > 0 && (
        <div className="mt-3 flex flex-wrap items-center gap-3">
          {modelList.map((m, i) => {
            const hidden = hiddenModels.has(m);
            return (
              <button
                key={m}
                onClick={() => toggleModel(m)}
                title={hidden ? "点击显示该模型" : "点击隐藏该模型"}
                className={`flex items-center gap-1.5 rounded-md px-1.5 py-0.5 transition-all hover:bg-slate-100 ${
                  hidden ? "opacity-40" : ""
                }`}
              >
                <span
                  className="h-0.5 w-4 rounded-full"
                  style={{ backgroundColor: TREND_LINE_COLORS[i % TREND_LINE_COLORS.length].stroke }}
                />
                <span className={`text-xs ${hidden ? "text-slate-400 line-through" : "text-slate-600"}`}>{m}</span>
              </button>
            );
          })}
        </div>
      )}

      {/* 图表 */}
      <div
        ref={containerRef}
        className="relative mt-4"
        style={{ minHeight: chartH + padding.top + padding.bottom }}
        onMouseMove={e => {
          if (data.length === 0) return;
          const rect = containerRef.current!.getBoundingClientRect();
          const mx = e.clientX - rect.left;
          const my = e.clientY - rect.top;
          // 只在图表区域内触发
          if (mx < padding.left || mx > containerW - padding.right) {
            setHoverBar(null);
            return;
          }
          const hi = Math.max(0, Math.min(Math.round((mx - padding.left) / stepX), hoursList.length - 1));
          const hour = hoursList[hi];
          const hd = visibleSeries
            .filter(s => {
              const p = s.points[hi];
              return p && (p.input > 0 || p.output > 0 || p.cached > 0);
            })
            .map(s => ({ model: s.model, input: s.points[hi].input, output: s.points[hi].output, cached: s.points[hi].cached }));
          setHoverBar({ x: mx, y: my, hour, data: hd });
        }}
        onMouseLeave={() => setHoverBar(null)}
      >
        {data.length === 0 ? (
          <div className="flex h-full items-center justify-center text-sm text-slate-400">
            暂无趋势数据
          </div>
        ) : (
          <svg
            width={containerW}
            height={chartH + padding.top + padding.bottom}
            className="overflow-visible"
          >
            {/* Y 轴网格线 + 标签 */}
            {yTickValues.map((v, i) => {
              const y = padding.top + chartH - (v / maxValue) * chartH;
              return (
                <g key={i}>
                  <line
                    x1={padding.left}
                    y1={y}
                    x2={containerW - padding.right}
                    y2={y}
                    stroke="#f1f5f9"
                    strokeWidth={1}
                  />
                  <text
                    x={padding.left - 10}
                    y={y + 4}
                    textAnchor="end"
                    className="fill-slate-400 text-[10px] tabular-nums"
                  >
                    {formatTick(v)}
                  </text>
                </g>
              );
            })}

            {/* 堆叠柱(按模型颜色分段叠加) */}
            {stackedBars.map((bar, bi) => {
              const barW = Math.max(Math.min(stepX * 0.7, 42), 3);
              const x = padding.left + bi * stepX - barW / 2;
              let acc = 0;
              return bar.segments.map((seg, si) => {
                const segH = (seg.value / maxValue) * chartH;
                const y = padding.top + chartH - ((acc + seg.value) / maxValue) * chartH;
                acc += seg.value;
                const color = TREND_LINE_COLORS[seg.colorIdx % TREND_LINE_COLORS.length];
                return (
                  <rect
                    key={`bar-${bi}-${si}`}
                    x={x}
                    y={y}
                    width={barW}
                    height={Math.max(segH, 0)}
                    fill={color.fill}
                  />
                );
              });
            })}

            {/* Hover:整根柱高亮 */}
            {hoverBar && clampedHoverIndex >= 0 && clampedHoverIndex < hoursList.length && (
              <g>
                <rect
                  x={Math.max(padding.left + clampedHoverIndex * stepX - stepX / 2, padding.left)}
                  y={padding.top}
                  width={stepX}
                  height={chartH}
                  fill="#3b82f6"
                  opacity={0.08}
                  pointerEvents="none"
                />
                {/* 柱顶基准线:指示该桶堆叠总高 */}
                <line
                  x1={Math.max(padding.left + clampedHoverIndex * stepX - stepX / 2, padding.left)}
                  y1={padding.top + chartH - (stackedBars[clampedHoverIndex]?.total ?? 0) / maxValue * chartH}
                  x2={Math.min(padding.left + clampedHoverIndex * stepX + stepX / 2, containerW - padding.right)}
                  y2={padding.top + chartH - (stackedBars[clampedHoverIndex]?.total ?? 0) / maxValue * chartH}
                  stroke="#64748b"
                  strokeWidth={1}
                  strokeDasharray="3 3"
                />
              </g>
            )}

            {/* X 轴标签 */}
            {hoursList.map((h, i) =>
              i % labelInterval === 0 ? (
                <text
                  key={h}
                  x={padding.left + i * stepX}
                  y={padding.top + chartH + 20}
                  textAnchor="middle"
                  className="fill-slate-400 text-[10px] tabular-nums"
                >
                  {formatHourShort(h)}
                </text>
              ) : null
            )}

            {/* X 轴基线 */}
            <line
              x1={padding.left}
              y1={padding.top + chartH}
              x2={containerW - padding.right}
              y2={padding.top + chartH}
              stroke="#e2e8f0"
              strokeWidth={1}
            />
          </svg>
        )}

        {/* Hover Tooltip */}
        {hoverBar && (
          <div
            className="pointer-events-none absolute z-10 min-w-[180px] max-w-xs rounded-xl border border-slate-200 bg-white/95 p-3 shadow-xl backdrop-blur-sm"
            style={{
              // 浮窗边缘翻转逻辑:默认显示在柱子右侧;若右侧空间不足
              // (鼠标位置 + 浮窗预估宽度超出容器),翻转到柱子左侧;
              // 两侧均不足时钳制在容器内,保证完整显示不溢出视域。
              left: (() => {
                const w = 220; // 浮窗预估宽度(min-w 180 + 内边距余量)
                const preferRight = hoverBar.x + 16 + w <= containerW;
                const left = preferRight ? hoverBar.x + 16 : hoverBar.x - 16 - w;
                return Math.max(8, Math.min(left, containerW - w - 8));
              })(),
              top: Math.max(hoverBar.y - 90, 8),
            }}
          >
            <div className="text-xs font-semibold text-slate-900">
              {formatHourLabel(hoverBar.hour)}
            </div>
            <div className="mt-1.5 space-y-1">
              {hoverBar.data.length === 0 ? (
                <div className="text-xs text-slate-400">无数据</div>
              ) : (
                <>
                  {/* 表头 */}
                  <div className="flex items-center gap-2 text-[10px] text-slate-400">
                    <span className="h-2 w-2" />
                    <span className="flex-1">模型</span>
                    {showInput && <span className="w-16 text-right">输入</span>}
                    {showOutput && <span className="w-16 text-right">输出</span>}
                    {showCached && <span className="w-16 text-right">缓存</span>}
                  </div>
                  {hoverBar.data.map((d) => {
                    const colorIdx = modelList.indexOf(d.model);
                    const color = TREND_LINE_COLORS[colorIdx % TREND_LINE_COLORS.length];
                    return (
                      <div key={d.model} className="flex items-center gap-2 text-xs">
                        <span
                          className="h-2 w-2 rounded-full"
                          style={{ backgroundColor: color.stroke }}
                        />
                        <span className="flex-1 text-slate-600">{d.model}</span>
                        {showInput && <span className="w-16 text-right font-medium tabular-nums text-slate-900">{formatNumber(d.input)}</span>}
                        {showOutput && <span className="w-16 text-right font-medium tabular-nums text-slate-900">{formatNumber(d.output)}</span>}
                        {showCached && <span className="w-16 text-right font-medium tabular-nums text-slate-900">{formatNumber(d.cached)}</span>}
                      </div>
                    );
                  })}
                  <div className="mt-1.5 border-t border-slate-100 pt-1.5 flex justify-between text-xs">
                    <span className="text-slate-400">合计</span>
                    <span className="flex gap-3">
                      {showInput && <span className="w-16 text-right font-semibold tabular-nums text-slate-900">{formatNumber(hoverBar.data.reduce((a, d) => a + d.input, 0))}</span>}
                      {showOutput && <span className="w-16 text-right font-semibold tabular-nums text-slate-900">{formatNumber(hoverBar.data.reduce((a, d) => a + d.output, 0))}</span>}
                      {showCached && <span className="w-16 text-right font-semibold tabular-nums text-slate-900">{formatNumber(hoverBar.data.reduce((a, d) => a + d.cached, 0))}</span>}
                    </span>
                  </div>
                </>
              )}
            </div>
          </div>
        )}
      </div>

      {/* 底部工具栏：指标开关（左）+ 区间汇总（右） */}
      <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-slate-100 pt-3">
        <div className="flex items-center gap-1.5">
          {([
            { label: "输入", dot: "bg-sky-400", active: showInput, onToggle: onToggleInput },
            { label: "输出", dot: "bg-violet-400", active: showOutput, onToggle: onToggleOutput },
            { label: "缓存", dot: "bg-emerald-400", active: showCached, onToggle: onToggleCached },
          ] as const).map(opt => (
            <button
              key={opt.label}
              onClick={opt.onToggle}
              className={`flex items-center gap-1.5 whitespace-nowrap rounded-full border px-3 py-1 text-xs font-medium transition-all ${
                opt.active
                  ? "border-slate-200 bg-white text-slate-700 shadow-sm"
                  : "border-transparent bg-transparent text-slate-300 hover:text-slate-500"
              }`}
            >
              <span className={`h-2 w-2 rounded-full ${opt.active ? opt.dot : "bg-slate-200"}`} />
              {opt.label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-4 text-[11px] text-slate-400">
          {showInput && <span className="flex items-center gap-1.5"><span className="h-2 w-2 rounded-full bg-sky-400" />输入 <b className="font-semibold tabular-nums text-slate-700">{formatNumber(visibleSeries.reduce((a, s) => a + s.points.reduce((x, p) => x + p.input, 0), 0))}</b></span>}
          {showOutput && <span className="flex items-center gap-1.5"><span className="h-2 w-2 rounded-full bg-violet-400" />输出 <b className="font-semibold tabular-nums text-slate-700">{formatNumber(visibleSeries.reduce((a, s) => a + s.points.reduce((x, p) => x + p.output, 0), 0))}</b></span>}
          {showCached && <span className="flex items-center gap-1.5"><span className="h-2 w-2 rounded-full bg-emerald-400" />缓存 <b className="font-semibold tabular-nums text-slate-700">{formatNumber(visibleSeries.reduce((a, s) => a + s.points.reduce((x, p) => x + p.cached, 0), 0))}</b></span>}
        </div>
      </div>
    </section>
  );
}
