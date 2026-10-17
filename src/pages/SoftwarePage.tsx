import { useCallback, useEffect, useState } from "react";
import { AppWindow, Plus, RefreshCw, Trash2, Pencil, Terminal, Globe, ScanSearch, Check } from "lucide-react";
import { softwareApi, serverApi } from "../lib/api";
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
  const [instances, setInstances] = useState<SoftwareInstance[]>([]);
  const [catalog, setCatalog] = useState<SoftwareConnectorInfo[]>([]);
  const [logs, setLogs] = useState<SoftwareToolLog[]>([]);
  const [mcpUrl, setMcpUrl] = useState<string>("");
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [error, setError] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [expandedTools, setExpandedTools] = useState<number | null>(null);
  const [toolPreviews, setToolPreviews] = useState<Record<number, SoftwareToolPreview[]>>({});

  // 本机扫描
  const [scanOpen, setScanOpen] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scanResults, setScanResults] = useState<ScannedApp[]>([]);
  const [scanSelected, setScanSelected] = useState<Set<string>>(new Set());
  const [scanDone, setScanDone] = useState<string[]>([]);

  const reload = useCallback(async () => {
    try {
      const [inst, cat, lg, status] = await Promise.all([
        softwareApi.getInstances(),
        softwareApi.getCatalog(),
        softwareApi.getLogs(undefined, 20),
        serverApi.getStatus().catch(() => null),
      ]);
      setInstances(inst);
      setCatalog(cat);
      setLogs(lg);
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
    setEditor(emptyEditor(connectorId, connectorSample(connectorId)));
  };

  const openEdit = (inst: SoftwareInstance) => {
    setError("");
    setEditor({
      id: inst.id,
      connectorId: inst.connector_id,
      name: inst.name,
      configText: JSON.stringify(inst.config, null, 2),
      approvalMode: inst.approval_mode,
      mcpEnabled: inst.mcp_enabled === 1,
    });
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

  return (
    <div className="space-y-5">
      {/* Section 头 */}
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-0 flex-1">
          <h2 className="flex items-center gap-2 text-sm font-medium text-slate-900">
            <AppWindow size={16} className="text-blue-500" /> 软件对接
          </h2>
          <p className="mt-0.5 text-xs text-slate-500">
            把本地软件的操作能力封装为 MCP 工具，AI Agent 连接 /mcp 即可操作软件
          </p>
        </div>
        <button className="action-secondary flex items-center gap-1.5" onClick={() => void runScan()}>
          <ScanSearch size={14} /> 扫描本机软件
        </button>
        <button className="action-secondary flex items-center gap-1.5" onClick={() => void reload()}>
          <RefreshCw size={14} /> 刷新
        </button>
        {catalog.length > 0 && (
          <button
            className="action-primary flex items-center gap-1.5"
            onClick={() => openCreate(catalog[0].id)}
          >
            <Plus size={14} /> 手动接入
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

      {/* 连接器市场 */}
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        {catalog.map((c) => (
          <div key={c.id} className="surface flex items-start gap-3 rounded-2xl p-4">
            <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-slate-200 bg-white">
              {c.id === "generic_http" ? (
                <Globe size={17} className="text-blue-500" />
              ) : c.id === "wechat" ? (
                <AppWindow size={17} className="text-emerald-600" />
              ) : (
                <Terminal size={17} className="text-teal-600" />
              )}
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="font-medium text-slate-900">{c.label}</span>
                <span className="rounded-full border border-slate-200 bg-white px-2 py-0.5 text-[10px] text-slate-500">
                  {c.platforms}
                </span>
              </div>
              <p className="mt-1 text-xs leading-relaxed text-slate-500">{c.description}</p>
            </div>
            <button className="action-secondary shrink-0" onClick={() => openCreate(c.id)}>
              接入
            </button>
          </div>
        ))}
      </div>

      {/* 实例列表 */}
      {instances.length === 0 ? (
        <div className="empty-state">
          <AppWindow size={28} className="mx-auto mb-2 text-slate-300" />
          <p>还没有接入任何软件。点击「扫描本机软件」或从上方选择连接器开始。</p>
        </div>
      ) : (
        <div className="space-y-3">
          {instances.map((inst) => (
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
                <select
                  className="rounded-xl border border-slate-200 bg-white px-2 py-1.5 text-xs text-slate-700"
                  value={inst.approval_mode}
                  onChange={(e) => void changeApproval(inst, e.target.value)}
                >
                  {APPROVAL_OPTIONS.map((o) => (
                    <option key={o.value} value={o.value}>
                      {o.label}
                    </option>
                  ))}
                </select>
                <button className="action-secondary" onClick={() => void showTools(inst)}>
                  {expandedTools === inst.id ? "收起工具" : "工具"}
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

      {/* 近期调用日志 */}
      {logs.length > 0 && (
        <div className="surface rounded-2xl p-4">
          <div className="mb-2 text-sm font-medium text-slate-900">近期调用</div>
          <div className="space-y-1 text-xs text-slate-500">
            {logs.map((l) => (
              <div key={l.id} className="flex items-center gap-2">
                <span
                  className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                    l.status === "ok"
                      ? "bg-emerald-500"
                      : l.status === "denied"
                        ? "bg-amber-500"
                        : "bg-rose-500"
                  }`}
                />
                <code className="font-mono text-[11px] text-slate-700">{l.tool}</code>
                <span>{l.status}</span>
                <span className="text-slate-400">{l.duration_ms}ms</span>
                {l.error && <span className="min-w-0 flex-1 truncate text-rose-500">{l.error}</span>}
                <span className="ml-auto shrink-0 text-slate-400">{l.created_at}</span>
              </div>
            ))}
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
                <select
                  className="w-full rounded-xl border border-slate-200 bg-white px-3 py-2 text-sm text-slate-900"
                  value={editor.approvalMode}
                  onChange={(e) => setEditor({ ...editor, approvalMode: e.target.value })}
                >
                  {APPROVAL_OPTIONS.map((o) => (
                    <option key={o.value} value={o.value}>
                      {o.label} — {o.hint}
                    </option>
                  ))}
                </select>
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
