import { useCallback, useEffect, useState } from "react";
import { AppWindow, Plus, RefreshCw, Trash2, Pencil, Terminal, Globe, ScanSearch, Check, Link as LinkIcon, FileJson, History, Activity, CircleHelp } from "lucide-react";
import { softwareApi, serverApi } from "../lib/api";
import { Select } from "../components/Select";
import type {
  SoftwareInstance,
  SoftwareConnectorInfo,
  SoftwareToolPreview,
  SoftwareToolLog,
  ScannedApp,
  CreateSoftwareInstanceInput,
} from "../types";

const APPROVAL_OPTIONS = [
  { value: "auto", label: "自动放行", hint: "所有工具直接执行" },
  { value: "confirm_high_risk", label: "高危需确认", hint: "高危写操作弹窗确认（推荐）" },
  { value: "confirm_all", label: "全部需确认", hint: "除只读外每次操作都确认" },
];

const riskColor = (risk: string) =>
  risk === "high_write"
    ? "bg-rose-50 text-rose-600 border-rose-200"
    : risk === "low_write"
      ? "bg-amber-50 text-amber-600 border-amber-200"
      : "bg-emerald-50 text-emerald-600 border-emerald-200";

/** ISO 时间 → 紧凑本地时间（当天只显示时分秒，跨天带月-日） */
const fmtTime = (iso: string) => {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const hm = d.toLocaleTimeString("zh-CN", {
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return hm;
  return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")} ${hm}`;
};

/** 单条调用日志行（实例日志弹窗 / 全局日志共用） */
function LogRow({ log }: { log: SoftwareToolLog }) {
  return (
    <div className="flex items-center gap-2 text-xs">
      <span
        className={`h-1.5 w-1.5 shrink-0 rounded-full ${
          log.status === "ok"
            ? "bg-emerald-500"
            : log.status === "denied"
              ? "bg-amber-500"
              : "bg-rose-500"
        }`}
      />
      <code className="shrink-0 font-mono text-[11px] text-slate-700">{log.tool}</code>
      <span
        className={`shrink-0 ${
          log.status === "ok"
            ? "text-slate-400"
            : log.status === "denied"
              ? "text-amber-600"
              : "text-rose-500"
        }`}
      >
        {log.status}
      </span>
      <span className="shrink-0 text-slate-400">{log.duration_ms}ms</span>
      {log.error && (
        <span className="min-w-0 flex-1 truncate text-rose-500" title={log.error}>
          {log.error}
        </span>
      )}
      <span className="ml-auto shrink-0 tabular-nums text-slate-400">{fmtTime(log.created_at)}</span>
    </div>
  );
}

/** 分页条（与「审计日志」页同款）：左侧计数，右侧 上一页/页码/下一页 */
function Pagination({
  page,
  totalPages,
  total,
  onPage,
  className,
}: {
  page: number;
  totalPages: number;
  total: number;
  onPage: (p: number) => void;
  className?: string;
}) {
  // 可点击页码：首尾页 + 当前页 ±2，其余折叠为省略号
  const items: (number | "…")[] = [];
  for (let i = 1; i <= totalPages; i++) {
    if (i === 1 || i === totalPages || Math.abs(i - page) <= 2) {
      items.push(i);
    } else if (items[items.length - 1] !== "…") {
      items.push("…");
    }
  }
  const gotoCls =
    "action-secondary disabled:cursor-not-allowed disabled:opacity-40";
  const gotoStyle = { padding: "6px 12px", fontSize: "13px" };
  return (
    <div
      className={`flex items-center justify-between gap-3 border-t border-slate-100 bg-white/60 px-4 py-2.5 ${className ?? ""}`}
    >
      <span className="whitespace-nowrap text-xs text-slate-400">
        共 {total} 条 · 第 {page} / {totalPages} 页
      </span>
      <div className="flex items-center gap-1">
        <button
          onClick={() => onPage(page - 1)}
          disabled={page <= 1}
          className={gotoCls}
          style={gotoStyle}
        >
          上一页
        </button>
        {items.map((item, idx) =>
          item === "…" ? (
            <span
              key={`ellipsis-${idx}`}
              className="select-none px-1.5 text-sm text-slate-400"
            >
              …
            </span>
          ) : (
            <button
              key={item}
              onClick={() => onPage(item)}
              className={`min-w-[30px] rounded-md px-1.5 py-1 text-[13px] transition-colors ${
                item === page
                  ? "bg-blue-500 font-medium text-white"
                  : "text-slate-500 hover:bg-slate-100 hover:text-slate-900"
              }`}
            >
              {item}
            </button>
          ),
        )}
        <button
          onClick={() => onPage(page + 1)}
          disabled={page >= totalPages}
          className={gotoCls}
          style={gotoStyle}
        >
          下一页
        </button>
      </div>
    </div>
  );
}

function Toggle({
  checked,
  onChange,
  disabled,
  title,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <button
      type="button"
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative h-5 w-9 shrink-0 rounded-full transition-colors disabled:opacity-40 ${
        checked ? "bg-blue-500" : "bg-slate-300"
      }`}
    >
      <span
        className={`absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all ${
          checked ? "left-[18px]" : "left-0.5"
        }`}
      />
    </button>
  );
}

interface EditorState {
  id: number | null;
  connectorId: string;
  name: string;
  configText: string;
  approvalMode: string;
  mcpEnabled: boolean;
}

const emptyEditor = (connectorId: string, sample: string): EditorState => ({
  id: null,
  connectorId,
  name: "",
  configText: sample,
  approvalMode: "confirm_high_risk",
  mcpEnabled: true,
});

export function SoftwareSection() {
  // 顶部双 Tab：已接入实例 / 近期调用
  const [viewTab, setViewTab] = useState<"instances" | "logs">("instances");
  const [instances, setInstances] = useState<SoftwareInstance[]>([]);
  const [catalog, setCatalog] = useState<SoftwareConnectorInfo[]>([]);
  const [logs, setLogs] = useState<SoftwareToolLog[]>([]);
  const [mcpUrl, setMcpUrl] = useState<string>("");
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [error, setError] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [expandedTools, setExpandedTools] = useState<number | null>(null);
  const [toolPreviews, setToolPreviews] = useState<Record<number, SoftwareToolPreview[]>>({});

  // 接入方式选择弹窗
  const [pickOpen, setPickOpen] = useState(false);
  // 使用帮助弹窗
  const [helpOpen, setHelpOpen] = useState(false);
  // 实例类型筛选 + 分页
  const [typeFilter, setTypeFilter] = useState<string>("all");
  const [page, setPage] = useState(1);
  // 日志分页
  const [logPage, setLogPage] = useState(1);
  const PAGE_SIZE = 6;
  const LOG_PAGE_SIZE = 20;

  // OpenAPI/Swagger 导入（generic_http 专用）
  const [oaSpec, setOaSpec] = useState("");
  const [oaBaseUrl, setOaBaseUrl] = useState("");
  const [oaImporting, setOaImporting] = useState(false);
  const [oaNotice, setOaNotice] = useState("");
  const [oaOpen, setOaOpen] = useState(false);

  // 本机扫描
  const [scanOpen, setScanOpen] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scanResults, setScanResults] = useState<ScannedApp[]>([]);
  const [scanSelected, setScanSelected] = useState<Set<string>>(new Set());
  const [scanDone, setScanDone] = useState<string[]>([]);

  // 分区折叠：接入新区默认无实例时展开（null = 跟随实例数）；调用日志默认收起
  // 单实例调用日志弹窗
  const [instLog, setInstLog] = useState<{ inst: SoftwareInstance; logs: SoftwareToolLog[] } | null>(null);

  // 类型筛选或数据刷新后回到第一页
  useEffect(() => {
    setPage(1);
  }, [typeFilter, instances.length]);

  const reload = useCallback(async () => {
    try {
      const [inst, cat, lg, status] = await Promise.all([
        softwareApi.getInstances(),
        softwareApi.getCatalog(),
        softwareApi.getLogs(undefined, 200),
        serverApi.getStatus().catch(() => null),
      ]);
      setInstances(inst);
      setCatalog(cat);
      setLogs(lg);
      setLogPage(1);
      if (status?.running && status?.url) setMcpUrl(`${status.url.replace(/\/$/, "")}/mcp`);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const connectorLabel = (id: string) => catalog.find((c) => c.id === id)?.label ?? id;
  const connectorSample = (id: string) =>
    JSON.stringify(catalog.find((c) => c.id === id)?.sample_config ?? {}, null, 2);

  const openCreate = (connectorId: string) => {
    setError("");
    setOaSpec("");
    setOaBaseUrl("");
    setOaNotice("");
    setOaOpen(false);
    setEditor(emptyEditor(connectorId, connectorSample(connectorId)));
  };

  const openEdit = (inst: SoftwareInstance) => {
    setError("");
    setOaSpec("");
    setOaBaseUrl("");
    setOaNotice("");
    setOaOpen(false);
    setEditor({
      id: inst.id,
      connectorId: inst.connector_id,
      name: inst.name,
      configText: JSON.stringify(inst.config, null, 2),
      approvalMode: inst.approval_mode,
      mcpEnabled: inst.mcp_enabled === 1,
    });
  };

  /** OpenAPI/Swagger 导入：服务端转换 generic_http 配置并回填编辑器 */
  const runOpenapiImport = async () => {
    if (!editor || !oaSpec.trim()) return;
    setError("");
    setOaNotice("");
    setOaImporting(true);
    try {
      const out = await softwareApi.importOpenapi(oaSpec, oaBaseUrl.trim() || undefined);
      setEditor({ ...editor, configText: JSON.stringify(out.config, null, 2) });
      setOaNotice(
        `导入成功：${out.tool_count} 个工具已生成配置` +
          (out.skipped.length ? `，${out.skipped.length} 项被跳过` : "") +
          "。请核对后保存。",
      );
    } catch (e) {
      setError(`OpenAPI 导入失败: ${String(e)}`);
    } finally {
      setOaImporting(false);
    }
  };

  const save = async () => {
    if (!editor) return;
    setError("");
    let config: Record<string, unknown>;
    try {
      config = JSON.parse(editor.configText);
    } catch {
      setError("配置不是合法 JSON");
      return;
    }
    setBusy(true);
    try {
      if (editor.id === null) {
        await softwareApi.create({
          connector_id: editor.connectorId,
          name: editor.name,
          config,
          mcp_enabled: editor.mcpEnabled,
          approval_mode: editor.approvalMode,
        });
      } else {
        await softwareApi.update({
          id: editor.id,
          name: editor.name,
          config,
          mcp_enabled: editor.mcpEnabled,
          approval_mode: editor.approvalMode,
        });
      }
      setEditor(null);
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (inst: SoftwareInstance) => {
    if (!window.confirm(`删除软件实例「${inst.name}」？其 MCP 工具将立即下线。`)) return;
    await softwareApi.delete(inst.id).catch((e) => setError(String(e)));
    await reload();
  };

  const toggleEnabled = async (inst: SoftwareInstance, v: boolean) => {
    setInstances((prev) => prev.map((i) => (i.id === inst.id ? { ...i, enabled: v ? 1 : 0 } : i)));
    await softwareApi.toggle(inst.id, v).catch((e) => setError(String(e)));
    await reload();
  };

  const toggleMcp = async (inst: SoftwareInstance, v: boolean) => {
    setInstances((prev) =>
      prev.map((i) => (i.id === inst.id ? { ...i, mcp_enabled: v ? 1 : 0 } : i)),
    );
    await softwareApi
      .update({ id: inst.id, mcp_enabled: v })
      .catch((e) => setError(String(e)));
    await reload();
  };

  const changeApproval = async (inst: SoftwareInstance, mode: string) => {
    setInstances((prev) =>
      prev.map((i) => (i.id === inst.id ? { ...i, approval_mode: mode } : i)),
    );
    await softwareApi.update({ id: inst.id, approval_mode: mode }).catch((e) => setError(String(e)));
  };

  const showTools = async (inst: SoftwareInstance) => {
    if (expandedTools === inst.id) {
      setExpandedTools(null);
      return;
    }
    setExpandedTools(inst.id);
    if (!toolPreviews[inst.id]) {
      const tools = await softwareApi.getTools(inst.id).catch(() => []);
      setToolPreviews((prev) => ({ ...prev, [inst.id]: tools }));
    }
  };

  /** 打开单实例调用日志弹窗 */
  const showInstanceLogs = async (inst: SoftwareInstance) => {
    setInstLog({ inst, logs: [] });
    const logs = await softwareApi.getLogs(inst.id, 50).catch(() => []);
    setInstLog({ inst, logs });
  };

  const runScan = async () => {
    setScanOpen(true);
    setError("");
    setScanDone([]);
    setScanning(true);
    setScanResults([]);
    try {
      const apps = await softwareApi.scan();
      setScanResults(apps);
      // 默认勾选命中预设的软件
      setScanSelected(new Set(apps.filter((a) => a.suggestion).map((a) => a.name)));
    } catch (e) {
      setError(String(e));
      setScanOpen(false);
    } finally {
      setScanning(false);
    }
  };

  const toggleScanItem = (name: string) => {
    setScanSelected((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const importSelected = async () => {
    setError("");
    const picked = scanResults.filter((a) => scanSelected.has(a.name));
    const existing = new Set(instances.map((i) => i.name));
    const created: string[] = [];
    let failed = 0;
    for (const app of picked) {
      if (existing.has(app.name)) continue; // 已接入的跳过
      const suggestion = app.suggestion ?? {
        connector_id: "generic_cli",
        config: {} as Record<string, unknown>,
      };
      const config = app.suggestion
        ? app.suggestion.config
        : defaultLaunchConfig(app.name, app.path);
      const input: CreateSoftwareInstanceInput = {
        connector_id: suggestion.connector_id,
        name: app.name,
        config,
        mcp_enabled: true,
        approval_mode: "confirm_high_risk",
      };
      try {
        await softwareApi.create(input);
        created.push(app.name);
      } catch {
        failed += 1;
      }
    }
    setScanDone(created);
    await reload();
    if (failed > 0) setError(`${failed} 个软件接入失败，可手动接入查看具体错误`);
  };

  // 派生：类型筛选 + 分页（实例与日志各自独立分页）
  const filteredInstances =
    typeFilter === "all" ? instances : instances.filter((i) => i.connector_id === typeFilter);
  const totalPages = Math.max(1, Math.ceil(filteredInstances.length / PAGE_SIZE));
  const currentPage = Math.min(page, totalPages);
  const pagedInstances = filteredInstances.slice(
    (currentPage - 1) * PAGE_SIZE,
    currentPage * PAGE_SIZE,
  );
  const logTotalPages = Math.max(1, Math.ceil(logs.length / LOG_PAGE_SIZE));
  const currentLogPage = Math.min(logPage, logTotalPages);
  const pagedLogs = logs.slice((currentLogPage - 1) * LOG_PAGE_SIZE, currentLogPage * LOG_PAGE_SIZE);

  const chipCls = (active: boolean) =>
    `rounded-full border px-3 py-1 text-xs transition-colors ${
      active
        ? "border-blue-200 bg-blue-50 text-blue-600"
        : "border-slate-200 bg-white text-slate-500 hover:text-slate-900"
    }`;
  const tabCls = (active: boolean) =>
    `flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-xl px-3 py-2 text-sm font-medium transition-all ${
      active
        ? "border border-blue-100 bg-white text-slate-900 shadow-[0_8px_18px_rgba(15,23,42,0.05)]"
        : "text-slate-500 hover:bg-white/70 hover:text-slate-900"
    }`;

  return (
    <div className="space-y-5">
      {/* Section 头 */}
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-0 flex-1">
          <h2 className="flex items-center gap-2 text-sm font-medium text-slate-900">
            <LinkIcon size={16} className="text-blue-500" /> Link 链接
            <button
              className="flex h-5 w-5 items-center justify-center rounded-full text-slate-400 transition-colors hover:bg-slate-100 hover:text-blue-600"
              onClick={() => setHelpOpen(true)}
              title="怎么使用？"
            >
              <CircleHelp size={14} />
            </button>
          </h2>
          <p className="mt-0.5 text-xs text-slate-500">
            通过本机软件、HTTP、微信等多种连接器，把操作能力封装为 MCP 工具，AI Agent 连接 /mcp 即可操作
          </p>
        </div>
        <button className="action-secondary flex items-center gap-1.5" onClick={() => void reload()}>
          <RefreshCw size={14} /> 刷新
        </button>
        {catalog.length > 0 && (
          <button
            className="action-primary flex items-center gap-1.5"
            onClick={() => setPickOpen(true)}
          >
            <Plus size={14} /> 接入软件
          </button>
        )}
      </div>

      {mcpUrl && (
        <div className="surface-soft flex items-center gap-2 rounded-2xl px-4 py-3 text-xs text-slate-500">
          <Terminal size={14} className="text-blue-500" />
          <span>Agent 连接端点</span>
          <code className="rounded-lg bg-white px-2 py-0.5 font-mono text-[12px] text-slate-700">
            {mcpUrl}
          </code>
          <span>（需 MCP Token 鉴权，工具名以 software__ 开头）</span>
        </div>
      )}

      {error && (
        <div className="rounded-2xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-600">
          {error}
        </div>
      )}

      {/* 双 Tab：已接入 / 近期调用 */}
      <div className="flex items-center gap-1.5">
        <button className={tabCls(viewTab === "instances")} onClick={() => setViewTab("instances")}>
          <AppWindow size={15} />
          已接入
          <span className="rounded-full bg-slate-100 px-1.5 py-0.5 text-[10px] tabular-nums text-slate-500">
            {instances.length}
          </span>
        </button>
        <button className={tabCls(viewTab === "logs")} onClick={() => setViewTab("logs")}>
          <Activity size={15} />
          近期调用
          {logs.length > 0 && (
            <span className="rounded-full bg-slate-100 px-1.5 py-0.5 text-[10px] tabular-nums text-slate-500">
              {logs.length}
            </span>
          )}
        </button>
      </div>

      {viewTab === "instances" && (
        <>
          {/* 类型筛选（只显示有实例的类型） */}
          {instances.length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5">
              <button className={chipCls(typeFilter === "all")} onClick={() => setTypeFilter("all")}>
                全部 {instances.length}
              </button>
              {catalog.map((c) => {
                const n = instances.filter((i) => i.connector_id === c.id).length;
                if (n === 0) return null;
                return (
                  <button
                    key={c.id}
                    className={chipCls(typeFilter === c.id)}
                    onClick={() => setTypeFilter(c.id)}
                  >
                    {c.label} {n}
                  </button>
                );
              })}
            </div>
          )}
          {instances.length === 0 ? (
            <div className="empty-state">
              <AppWindow size={28} className="mx-auto mb-2 text-slate-300" />
              <p>还没有接入任何软件。点击右上角「接入软件」选择接入方式开始。</p>
            </div>
          ) : filteredInstances.length === 0 ? (
            <div className="empty-state">
              <AppWindow size={28} className="mx-auto mb-2 text-slate-300" />
              <p>该类型下暂无实例。</p>
            </div>
          ) : (
          <div className="space-y-3">
            {pagedInstances.map((inst) => (
              <div key={inst.id} className="surface rounded-2xl p-4">
                <div className="flex flex-wrap items-center gap-3">
                  <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-slate-200 bg-white">
                    {inst.connector_id === "generic_http" ? (
                      <Globe size={17} className="text-blue-500" />
                    ) : inst.connector_id === "wechat" ? (
                      <AppWindow size={17} className="text-emerald-600" />
                    ) : (
                      <Terminal size={17} className="text-teal-600" />
                    )}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="font-medium text-slate-900">{inst.name}</span>
                      <span className="rounded-full border border-slate-200 bg-white px-2 py-0.5 text-[10px] text-slate-500">
                        {connectorLabel(inst.connector_id)}
                      </span>
                      {inst.enabled !== 1 && (
                        <span className="rounded-full border border-slate-200 bg-slate-100 px-2 py-0.5 text-[10px] text-slate-500">
                          已停用
                        </span>
                      )}
                    </div>
                    <div className="mt-0.5 text-[11px] text-slate-400">实例 #{inst.id}</div>
                  </div>

                  <label className="flex items-center gap-1.5 text-xs text-slate-500">
                    实例
                    <Toggle checked={inst.enabled === 1} onChange={(v) => void toggleEnabled(inst, v)} />
                  </label>
                  <label
                    className="flex items-center gap-1.5 text-xs text-slate-500"
                    title="开启后该实例工具出现在 /mcp tools/list"
                  >
                    MCP
                    <Toggle
                      checked={inst.mcp_enabled === 1}
                      onChange={(v) => void toggleMcp(inst, v)}
                    />
                  </label>
                  <Select
                    size="sm"
                    className="w-36"
                    ariaLabel="审批策略"
                    value={inst.approval_mode}
                    onChange={(v) => void changeApproval(inst, v)}
                    options={APPROVAL_OPTIONS.map((o) => ({ value: o.value, label: o.label }))}
                  />
                  <button className="action-secondary" onClick={() => void showTools(inst)}>
                    {expandedTools === inst.id ? "收起工具" : "工具"}
                  </button>
                  <button
                    className="flex h-8 w-8 items-center justify-center rounded-xl border border-slate-200 bg-white text-slate-500 hover:text-blue-600"
                    onClick={() => void showInstanceLogs(inst)}
                    title="该实例的调用日志"
                  >
                    <History size={14} />
                  </button>
                  <button
                    className="flex h-8 w-8 items-center justify-center rounded-xl border border-slate-200 bg-white text-slate-500 hover:text-blue-600"
                    onClick={() => openEdit(inst)}
                    title="编辑"
                  >
                    <Pencil size={14} />
                  </button>
                  <button
                    className="flex h-8 w-8 items-center justify-center rounded-xl border border-slate-200 bg-white text-slate-500 hover:text-rose-600"
                    onClick={() => void remove(inst)}
                    title="删除"
                  >
                    <Trash2 size={14} />
                  </button>
                </div>

                {expandedTools === inst.id && (
                  <div className="mt-3 space-y-1.5 rounded-2xl border border-slate-200 bg-slate-50 p-3">
                    {(toolPreviews[inst.id] ?? []).map((t) => (
                      <div key={t.name} className="flex items-start gap-2 text-xs">
                        <span
                          className={`shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] ${riskColor(t.risk)}`}
                        >
                          {t.risk_label}
                        </span>
                        <code className="shrink-0 font-mono text-[11px] text-slate-700">
                          {t.mcp_name}
                        </code>
                        <span className="min-w-0 flex-1 text-slate-500">{t.description}</span>
                      </div>
                    ))}
                    {(toolPreviews[inst.id] ?? []).length === 0 && (
                      <div className="text-xs text-slate-400">无工具（检查配置）</div>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}

          {/* 实例分页 */}
          {totalPages > 1 && (
            <Pagination
              page={currentPage}
              totalPages={totalPages}
              total={filteredInstances.length}
              onPage={setPage}
              className="rounded-2xl border border-slate-200 shadow-[0_1px_2px_rgba(15,23,42,0.03)]"
            />
          )}
        </>
      )}

      {viewTab === "logs" && (
        <div className="surface rounded-2xl p-4">
          {logs.length === 0 ? (
            <div className="text-xs text-slate-400">
              暂无调用记录。通过 /mcp 调用软件工具后在这里查看。
            </div>
          ) : (
            <>
              <div className="space-y-1.5">
                {pagedLogs.map((l) => (
                  <LogRow key={l.id} log={l} />
                ))}
              </div>
              {logTotalPages > 1 && (
                <Pagination
                  page={currentLogPage}
                  totalPages={logTotalPages}
                  total={logs.length}
                  onPage={setLogPage}
                  className="mt-3 rounded-xl border border-slate-100"
                />
              )}
            </>
          )}
        </div>
      )}

      {/* 单实例调用日志弹窗 */}
      {instLog && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-6"
          onClick={() => setInstLog(null)}
        >
          <div
            className="surface flex max-h-[80vh] w-full max-w-2xl flex-col overflow-hidden rounded-3xl p-6"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="mb-3 flex items-center gap-2">
              <History size={18} className="text-blue-500" />
              <h2 className="text-base font-medium text-slate-900">调用日志</h2>
              <span className="truncate text-xs text-slate-400">{instLog.inst.name}</span>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto rounded-2xl border border-slate-100 p-3">
              {instLog.logs.length === 0 ? (
                <div className="p-6 text-center text-xs text-slate-400">
                  暂无该实例的调用记录
                </div>
              ) : (
                <div className="space-y-1.5">
                  {instLog.logs.map((l) => (
                    <LogRow key={l.id} log={l} />
                  ))}
                </div>
              )}
            </div>
            <div className="mt-4 flex justify-end">
              <button className="action-secondary" onClick={() => setInstLog(null)}>
                关闭
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 使用帮助弹窗 */}
      {helpOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-6"
          onClick={() => setHelpOpen(false)}
        >
          <div
            className="surface max-h-[85vh] w-full max-w-2xl overflow-y-auto rounded-3xl p-6"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="mb-1 flex items-center gap-2">
              <CircleHelp size={18} className="text-blue-500" />
              <h2 className="text-base font-medium text-slate-900">Link 使用指南</h2>
            </div>
            <p className="mb-4 text-xs text-slate-500">
              Link 把本机软件、HTTP 服务、微信等外部能力封装为 MCP 工具，让 AI Agent 像调用内置技能一样操作它们。
            </p>

            <div className="space-y-4 text-sm leading-relaxed text-slate-600">
              <div className="rounded-2xl border border-slate-100 bg-slate-50 p-4">
                <div className="mb-1 font-medium text-slate-900">
                  第 1 步 · 接入软件（建立连接）
                </div>
                点击右上角「<b>接入软件</b>」选择接入方式，或使用「<b>扫描本机软件</b>」自动识别已安装的应用：
                <ul className="mt-1.5 list-disc space-y-1 pl-5 text-[13px]">
                  <li>
                    <b>通用本地 HTTP</b>：对接自带 HTTP API 的软件（Ollama、下载器、自研服务等）。支持粘贴
                    <b> OpenAPI / Swagger</b> 文档自动生成全部端点配置，免手写 JSON。
                  </li>
                  <li><b>通用命令行</b>：把 CLI 命令 / AppleScript / PowerShell 脚本登记为工具。</li>
                  <li><b>MCP 聚合网关</b>：代理社区现成的 MCP Server，其工具自动并入。</li>
                  <li><b>微信</b>（实验性）：给联系人发文本等辅助功能。</li>
                </ul>
              </div>

              <div className="rounded-2xl border border-slate-100 bg-slate-50 p-4">
                <div className="mb-1 font-medium text-slate-900">
                  第 2 步 · 配置安全策略（控制权限）
                </div>
                每个实例卡片上可以调整：
                <ul className="mt-1.5 list-disc space-y-1 pl-5 text-[13px]">
                  <li><b>MCP 开关</b>：控制该实例的工具是否暴露给 AI Agent。</li>
                  <li>
                    <b>审批策略</b>：自动放行 / 高危需确认（推荐）/ 全部需确认。Agent
                    执行写入类操作时会按策略弹窗向你确认。
                  </li>
                  <li><b>工具</b>按钮：预览该实例生成了哪些 MCP 工具及风险级别。</li>
                </ul>
              </div>

              <div className="rounded-2xl border border-slate-100 bg-slate-50 p-4">
                <div className="mb-1 font-medium text-slate-900">
                  第 3 步 · 连接 AI Agent（开始使用）
                </div>
                把上方「<b>Agent 连接端点</b>」（<code className="rounded bg-white px-1 font-mono text-[12px]">http://127.0.0.1:8777/mcp</code>
                ）配置到任何支持 MCP 的 AI Agent（Claude、Cursor 等）。Agent 会自动发现形如
                <code className="mx-0.5 rounded bg-white px-1 font-mono text-[12px]">software__实例名__工具名</code>
                的工具，之后直接用自然语言指挥即可，例如「帮我在 Todo 里加一条明天开会」。
              </div>

              <div className="rounded-2xl border border-slate-100 bg-slate-50 p-4">
                <div className="mb-1 font-medium text-slate-900">排查问题（看调用记录）</div>
                <ul className="mt-1.5 list-disc space-y-1 pl-5 text-[13px]">
                  <li><b>近期调用</b> Tab：所有软件工具的调用记录（成功 / 拒绝 / 失败、耗时、错误信息）。</li>
                  <li>实例卡片上的 <b>时钟图标</b>：只看该实例自己的调用日志。</li>
                  <li>接入后 Agent 里看不到工具？检查实例的 <b>MCP 开关</b>是否打开、实例是否启用。</li>
                </ul>
              </div>
            </div>

            <div className="mt-4 flex justify-end">
              <button className="action-primary" onClick={() => setHelpOpen(false)}>
                知道了
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 接入方式选择弹窗 */}
      {pickOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-6"
          onClick={() => setPickOpen(false)}
        >
          <div
            className="surface w-full max-w-lg overflow-hidden rounded-3xl p-6"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="mb-1 flex items-center gap-2">
              <LinkIcon size={18} className="text-blue-500" />
              <h2 className="text-base font-medium text-slate-900">选择接入方式</h2>
            </div>
            <p className="mb-4 text-xs text-slate-500">
              选择连接器类型后进入配置；也可以扫描本机已安装的软件自动识别。
            </p>
            <div className="space-y-2">
              {catalog.map((c) => (
                <button
                  key={c.id}
                  className="surface-soft flex w-full items-start gap-3 rounded-2xl p-4 text-left transition-shadow hover:shadow-[0_8px_18px_rgba(15,23,42,0.06)]"
                  onClick={() => {
                    setPickOpen(false);
                    openCreate(c.id);
                  }}
                >
                  <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-slate-200 bg-white">
                    {c.id === "generic_http" ? (
                      <Globe size={17} className="text-blue-500" />
                    ) : c.id === "wechat" ? (
                      <AppWindow size={17} className="text-emerald-600" />
                    ) : (
                      <Terminal size={17} className="text-teal-600" />
                    )}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center gap-2">
                      <span className="font-medium text-slate-900">{c.label}</span>
                      <span className="rounded-full border border-slate-200 bg-white px-2 py-0.5 text-[10px] text-slate-500">
                        {c.platforms}
                      </span>
                    </span>
                    <span className="mt-1 block text-xs leading-relaxed text-slate-500">
                      {c.description}
                    </span>
                  </span>
                </button>
              ))}
            </div>
            <div className="mt-4 border-t border-slate-100 pt-4 text-center">
              <button
                className="action-secondary mx-auto flex items-center gap-1.5 text-xs"
                onClick={() => {
                  setPickOpen(false);
                  void runScan();
                }}
              >
                <ScanSearch size={14} /> 扫描本机软件
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 扫描结果弹窗 */}
      {scanOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-6">
          <div className="surface flex max-h-[85vh] w-full max-w-2xl flex-col overflow-hidden rounded-3xl p-6">
            <div className="mb-3 flex items-center gap-2">
              <ScanSearch size={18} className="text-blue-500" />
              <h2 className="text-base font-medium text-slate-900">扫描本机软件</h2>
              <span className="text-xs text-slate-400">
                {scanning ? "扫描中..." : `共 ${scanResults.length} 个，已选 ${scanSelected.size} 个`}
              </span>
            </div>

            {scanDone.length > 0 && (
              <div className="mb-3 flex items-center gap-2 rounded-2xl border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700">
                <Check size={14} />
                已接入：{scanDone.join("、")}
              </div>
            )}

            <div className="min-h-0 flex-1 overflow-y-auto rounded-2xl border border-slate-100">
              {scanning && (
                <div className="p-8 text-center text-sm text-slate-400">正在枚举本机软件...</div>
              )}
              {!scanning && scanResults.length === 0 && (
                <div className="p-8 text-center text-sm text-slate-400">未扫描到软件</div>
              )}
              {scanResults.map((app) => {
                const selected = scanSelected.has(app.name);
                return (
                  <label
                    key={app.path}
                    className="flex cursor-pointer items-center gap-3 border-b border-slate-50 px-4 py-2.5 last:border-0 hover:bg-slate-50"
                  >
                    <input
                      type="checkbox"
                      className="h-4 w-4 accent-blue-500"
                      checked={selected}
                      onChange={() => toggleScanItem(app.name)}
                    />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="truncate text-sm text-slate-900">{app.name}</span>
                        {app.suggestion && (
                          <span className="shrink-0 rounded-full border border-blue-200 bg-blue-50 px-1.5 py-0.5 text-[10px] text-blue-600">
                            {app.suggestion.connector_label}
                          </span>
                        )}
                      </div>
                      <div className="truncate text-[11px] text-slate-400" title={app.path}>
                        {app.suggestion ? app.suggestion.hint : app.path}
                      </div>
                    </div>
                    <span className="shrink-0 text-[10px] text-slate-400">{app.source}</span>
                  </label>
                );
              })}
            </div>

            <div className="mt-4 flex justify-between">
              <button
                className="action-secondary text-xs"
                onClick={() => setScanSelected(new Set(scanResults.map((a) => a.name)))}
              >
                全选
              </button>
              <div className="flex gap-2">
                <button className="action-secondary" onClick={() => setScanOpen(false)}>
                  关闭
                </button>
                <button
                  className="action-primary"
                  disabled={scanning || scanSelected.size === 0}
                  onClick={() => void importSelected()}
                >
                  接入选中（{scanSelected.size}）
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* 新建/编辑弹窗 */}
      {editor && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-6">
          <div className="surface max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-3xl p-6">
            <div className="mb-4 flex items-center gap-2">
              <AppWindow size={18} className="text-blue-500" />
              <h2 className="text-base font-medium text-slate-900">
                {editor.id === null ? "接入软件" : "编辑软件实例"}
              </h2>
              <span className="rounded-full border border-slate-200 bg-white px-2 py-0.5 text-[10px] text-slate-500">
                {connectorLabel(editor.connectorId)}
              </span>
            </div>

            <div className="space-y-4">
              <div>
                <label className="mb-1 block text-xs font-medium text-slate-600">实例名称</label>
                <input
                  className="w-full rounded-xl border border-slate-200 bg-white px-3 py-2 text-sm text-slate-900 outline-none focus:border-blue-400"
                  placeholder="如：微信 / 本地 Ollama"
                  value={editor.name}
                  onChange={(e) => setEditor({ ...editor, name: e.target.value })}
                />
              </div>

              {editor.connectorId === "generic_http" && (
                <div className="rounded-2xl border border-slate-200 bg-slate-50 p-3">
                  <button
                    className="flex w-full items-center justify-between text-xs font-medium text-slate-600"
                    onClick={() => setOaOpen((v) => !v)}
                  >
                    <span className="flex items-center gap-1.5">
                      <FileJson size={14} className="text-blue-500" />
                      从 OpenAPI / Swagger 导入（免手写 JSON）
                    </span>
                    <span className="text-slate-400">{oaOpen ? "收起" : "展开"}</span>
                  </button>
                  {oaOpen && (
                    <div className="mt-3 space-y-2">
                      <textarea
                        className="h-32 w-full rounded-xl border border-slate-200 bg-white px-3 py-2 font-mono text-[11px] leading-relaxed text-slate-900 outline-none focus:border-blue-400"
                        placeholder="粘贴 OpenAPI 3.x / Swagger 2.0 文档全文（JSON 或 YAML），或让 AI Agent 通过 /mcp 的 software_import_openapi 工具自动完成"
                        value={oaSpec}
                        onChange={(e) => setOaSpec(e.target.value)}
                        spellCheck={false}
                      />
                      <div className="flex items-center gap-2">
                        <input
                          className="flex-1 rounded-xl border border-slate-200 bg-white px-3 py-1.5 text-xs text-slate-900 outline-none focus:border-blue-400"
                          placeholder="可选：base_url 覆盖（规范内 servers 为相对地址时必填）"
                          value={oaBaseUrl}
                          onChange={(e) => setOaBaseUrl(e.target.value)}
                        />
                        <button
                          className="action-secondary whitespace-nowrap text-xs"
                          disabled={oaImporting || !oaSpec.trim()}
                          onClick={() => void runOpenapiImport()}
                        >
                          {oaImporting ? "导入中..." : "导入生成配置"}
                        </button>
                      </div>
                      {oaNotice && (
                        <p className="text-xs text-emerald-600">{oaNotice}</p>
                      )}
                    </div>
                  )}
                </div>
              )}

              <div>
                <div className="mb-1 flex items-center justify-between">
                  <label className="text-xs font-medium text-slate-600">配置（JSON）</label>
                  <button
                    className="text-xs text-blue-600 hover:underline"
                    onClick={() =>
                      setEditor({ ...editor, configText: connectorSample(editor.connectorId) })
                    }
                  >
                    插入示例配置
                  </button>
                </div>
                <textarea
                  className="h-64 w-full rounded-xl border border-slate-200 bg-white px-3 py-2 font-mono text-[12px] leading-relaxed text-slate-900 outline-none focus:border-blue-400"
                  value={editor.configText}
                  onChange={(e) => setEditor({ ...editor, configText: e.target.value })}
                  spellCheck={false}
                />
              </div>

              <div>
                <label className="mb-1 block text-xs font-medium text-slate-600">审批策略</label>
                <Select
                  className="w-full"
                  ariaLabel="审批策略"
                  value={editor.approvalMode}
                  onChange={(v) => setEditor({ ...editor, approvalMode: v })}
                  options={APPROVAL_OPTIONS.map((o) => ({ value: o.value, label: `${o.label} — ${o.hint}` }))}
                />
              </div>

              <label className="flex items-center gap-2 text-sm text-slate-700">
                <Toggle
                  checked={editor.mcpEnabled}
                  onChange={(v) => setEditor({ ...editor, mcpEnabled: v })}
                />
                立即在 MCP 中暴露该实例的工具
              </label>

              {error && (
                <div className="rounded-xl border border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-600">
                  {error}
                </div>
              )}

              <div className="flex justify-end gap-2">
                <button className="action-secondary" onClick={() => setEditor(null)}>
                  取消
                </button>
                <button className="action-primary" disabled={busy} onClick={() => void save()}>
                  {busy ? "保存中..." : "保存"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** 未命中预设时的默认配置：跨平台「启动软件」命令。 */
function defaultLaunchConfig(name: string, path: string): Record<string, unknown> {
  const isMac = navigator.userAgent.includes("Mac");
  const isWin = navigator.userAgent.includes("Win");
  if (isMac) {
    return {
      commands: [
        { tool: "launch", description: `启动 ${name}`, program: "open", args: ["-a", name], risk: "read_only", timeout_secs: 15 },
      ],
    };
  }
  if (isWin) {
    return {
      commands: [
        { tool: "launch", description: `启动 ${name}`, program: "cmd", args: ["/c", "start", "", path], risk: "read_only", timeout_secs: 15 },
      ],
    };
  }
  return {
    commands: [
      { tool: "launch", description: `启动 ${name}`, program: "xdg-open", args: [path], risk: "read_only", timeout_secs: 15 },
    ],
  };
}
