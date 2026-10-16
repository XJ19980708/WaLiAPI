import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { apiKeyApi, authApi, channelApi, statsApi } from "../lib/api";
import type { ApiKey, AuthAccount, Channel, RouteFlow } from "../types";
import { formatNumber, formatDuration } from "../lib/constants";
import { listen } from "../lib/runtime";
import { Activity, Radio, Key, RefreshCw, Boxes, Gauge, ArrowRight, Power } from "lucide-react";

/**
 * 网关路由动态图（可观测性 P4）——嵌入仪表盘的卡片。
 *
 * 三栏布局：左侧调用方（API Key），中间 WaLiAPI 网关，右侧上游渠道。
 * 连线为铺底 SVG 贝塞尔曲线，实时请求粒子沿 Key → 网关 → 渠道路径飞行
 * （数据面事件经 EventSink 桥推到前端）。右侧渠道节点附模型映射摘要与
 * 近 24h 流量（请求数 / 平均首字耗时），状态着色区分健康/探测失败。
 *
 * 渠道节点的映射信息来自 channelApi.getAll（model_mapping），流量来自
 * statsApi.getRouteFlow；两者独立刷新，任一失败均优雅降级。
 */

// ─── 事件载荷（与后端 emit 的 payload 对齐）────────────────────────────────

interface RouteEventPayload {
  api_key_name?: string | null;
  channel_name?: string | null;
  model?: string | null;
  status?: number | null;
  ts?: number;
}

interface Particle {
  id: number;
  keyName: string;
  channelName: string | null;
  /** 请求的下游模型名（start/done 事件均有），用于模型映射反查候选渠道。 */
  modelName: string | null;
  /** inflight = Key→网关；outbound = 网关→渠道；done = 完成（路径高亮 lingering 后消失） */
  phase: "inflight" | "outbound" | "done";
  startedAt: number;
  failed: boolean;
}

const INFLIGHT_MS = 700;
const OUTBOUND_MS = 900;
const DONE_MS = 600;
/** done 相位后路径高亮多停留的时间：粒子消失后连线仍点亮，肉眼才追得上。 */
const LINGER_MS = 1600;

let particleSeq = 0;

// ─── 主组件 ──────────────────────────────────────────────────────────────────

export function RouteMapCard() {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [accounts, setAccounts] = useState<AuthAccount[]>([]);
  const [flow, setFlow] = useState<RouteFlow[]>([]);
  const [particles, setParticles] = useState<Particle[]>([]);
  const [loading, setLoading] = useState(true);
  const [liveEvents, setLiveEvents] = useState(0);

  const particlesRef = useRef<Particle[]>([]);
  particlesRef.current = particles;

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [keysData, channelsData, accountsData, flowData] = await Promise.all([
        apiKeyApi.getAll(),
        channelApi.getAll(),
        authApi.accountsList().catch(() => [] as AuthAccount[]),
        statsApi.getRouteFlow(24).catch(() => [] as RouteFlow[]),
      ]);
      setKeys(keysData);
      setChannels(channelsData);
      setAccounts(accountsData);
      setFlow(flowData);
    } catch {
      // 静默降级：页面仍可展示静态拓扑
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 定时刷新流量数据（30s，与仪表盘同节奏）
  useEffect(() => {
    const timer = setInterval(() => {
      statsApi.getRouteFlow(24).then(setFlow).catch(() => {});
    }, 30000);
    return () => clearInterval(timer);
  }, []);

  // 数据面事件订阅：驱动粒子飞行。事件桥在桌面/Web 两种模式统一可用。
  useEffect(() => {
    const unsubs: Array<() => void> = [];
    const push = (
      keyName: string | null | undefined,
      channelName: string | null | undefined,
      failed: boolean,
      phase: Particle["phase"] = "inflight",
      modelName?: string | null,
    ) => {
      const p: Particle = {
        id: ++particleSeq,
        keyName: keyName || "未知调用方",
        channelName: channelName || null,
        modelName: modelName ?? null,
        phase,
        startedAt: Date.now(),
        failed,
      };
      setParticles(prev => [...prev.slice(-40), p]);
      setLiveEvents(n => n + 1);
    };

    // 后端事件名：gw_request_start / gw_request_done（event_bridge 统一出口）。
    void listen<RouteEventPayload>("gw_request_start", ({ payload }) => {
      push(payload?.api_key_name, null, false, "inflight", payload?.model);
    }).then(unsub => unsubs.push(unsub));
    // done 事件直接从出站相位开始：请求已经完成，不该再重放一遍入站动画，
    // 否则网关→渠道段被推迟 700ms 才出现，肉眼基本追不上。
    void listen<RouteEventPayload>("gw_request_done", ({ payload }) => {
      const failed = payload?.status != null && (payload.status < 200 || payload.status >= 300);
      push(payload?.api_key_name, payload?.channel_name, failed, "outbound", payload?.model);
    }).then(unsub => unsubs.push(unsub));

    return () => unsubs.forEach(unsub => unsub());
  }, []);

  // 粒子相位推进：rAF 按时间戳流转 inflight → outbound → done → 移除。
  // 有粒子时持续重绘（粒子位置是时间函数，需要每帧刷新）。
  // done 相位之后额外保留 LINGER_MS：这段时间粒子已消失，但路径高亮仍点亮。
  useEffect(() => {
    if (particles.length === 0) return;
    let raf = 0;
    const tick = () => {
      const now = Date.now();
      setParticles(prev => {
        const next: Particle[] = [];
        for (const p of prev) {
          const elapsed = now - p.startedAt;
          if (p.phase === "inflight") {
            if (elapsed < INFLIGHT_MS) next.push(p);
            else if (elapsed < INFLIGHT_MS + OUTBOUND_MS) next.push({ ...p, phase: "outbound" });
            else next.push({ ...p, phase: "done" });
          } else if (p.phase === "outbound") {
            if (elapsed < INFLIGHT_MS + OUTBOUND_MS) next.push(p);
            else next.push({ ...p, phase: "done" });
          } else {
            // done：粒子本体立即不可见，仅作为路径高亮信号保留 LINGER_MS
            if (elapsed < INFLIGHT_MS + OUTBOUND_MS + DONE_MS + LINGER_MS) next.push(p);
          }
        }
        return next;
      });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [particles.length > 0]);

  const flowByChannel = useMemo(() => {
    const map = new Map<string, RouteFlow>();
    for (const row of flow) map.set(row.channel_id, row);
    return map;
  }, [flow]);

  // 渠道按调度顺序排序：优先级升序（数字小者优先生效）→ 权重降序（同优先级内
  // 权重大者更常被选中）→ 名称稳定排序。Auth 账号同规则，且手动 sort_order 优先。
  const enabledChannels = useMemo(() => {
    return channels
      .filter(c => c.status === 1)
      .sort((a, b) =>
        a.priority - b.priority ||
        b.weight - a.weight ||
        a.name.localeCompare(b.name),
      );
  }, [channels]);
  const enabledAccounts = useMemo(() => {
    return accounts
      .filter(a => !a.disabled)
      .sort((a, b) =>
        (b.sort_order ?? 0) - (a.sort_order ?? 0) ||
        a.priority - b.priority ||
        b.weight - a.weight ||
        a.label.localeCompare(b.label),
      );
  }, [accounts]);
  const upstreamTotal = enabledChannels.length + enabledAccounts.length;

  // 最近调用模型（按 API Key）：来自实时事件的 model 字段，展示「这个 Key
  // 刚才用啥模型调进来的」。会话级状态，刷新页面后清空。
  const [recentModelByKey, setRecentModelByKey] = useState<Map<string, string>>(new Map());
  useEffect(() => {
    if (particles.length === 0) return;
    const latest = particles[particles.length - 1];
    if (!latest.modelName) return;
    setRecentModelByKey(prev => {
      if (prev.get(latest.keyName) === latest.modelName) return prev;
      const next = new Map(prev);
      next.set(latest.keyName, latest.modelName!);
      return next;
    });
  }, [particles]);

  // 映射开关（乐观更新 + 失败回滚）：与渠道页/Auth 页同一套 API。
  const toggleChannelMapping = useCallback(
    async (channel: Channel, name: string, target: string, currentlyOff: boolean) => {
      const current = channel.model_mapping_disabled ?? [];
      const next = currentlyOff
        ? current.filter(d => !(d[0] === name && d[1] === target))
        : [...current.filter(d => !(d[0] === name && d[1] === target)), [name, target]];
      setChannels(prev =>
        prev.map(c => (c.id === channel.id ? { ...c, model_mapping_disabled: next } : c)),
      );
      try {
        await channelApi.update({ id: channel.id, model_mapping_disabled: next });
      } catch (e) {
        setChannels(prev =>
          prev.map(c => (c.id === channel.id ? { ...c, model_mapping_disabled: current } : c)),
        );
        console.error("切换映射状态失败:", e);
      }
    },
    [],
  );

  const toggleAccountMapping = useCallback(
    async (account: AuthAccount, name: string, target: string, currentlyOff: boolean) => {
      const current = account.model_mapping_disabled ?? [];
      const next = currentlyOff
        ? current.filter(d => !(d[0] === name && d[1] === target))
        : [...current.filter(d => !(d[0] === name && d[1] === target)), [name, target]];
      setAccounts(prev =>
        prev.map(a => (a.id === account.id ? { ...a, model_mapping_disabled: next } : a)),
      );
      try {
        await authApi.updateMappingDisabled(account.id, next);
      } catch (e) {
        setAccounts(prev =>
          prev.map(a => (a.id === account.id ? { ...a, model_mapping_disabled: current } : a)),
        );
        console.error("切换映射状态失败:", e);
      }
    },
    [],
  );

  // 模型 → 渠道名列表 反查表：请求开始时只有模型名（尚未路由），据此点亮
  // 所有可能承接该模型的渠道（models 直含 或 model_mapping 的 key/值命中）。
  // Auth 账号同样参与（models 列表 / model_mapping）。
  const modelCandidates = useMemo(() => {
    const map = new Map<string, Set<string>>();
    const addCandidate = (model: string, name: string) => {
      const key = model.trim().toLowerCase();
      if (!key) return;
      if (!map.has(key)) map.set(key, new Set());
      map.get(key)!.add(name);
    };
    const mappingHits = (
      mapping: Record<string, string | string[]> | undefined,
      disabled: unknown,
      name: string,
    ) => {
      if (!mapping) return;
      // 被关闭的映射对不参与反查
      const off = new Set(
        (Array.isArray(disabled) ? disabled : [])
          .map(pair => (Array.isArray(pair) ? `${pair[0]}\u0000${pair[1]}` : null))
          .filter((v): v is string => v != null),
      );
      for (const [from, to] of Object.entries(mapping)) {
        const toList = Array.isArray(to) ? to : [to];
        for (const t of toList) {
          if (off.has(`${from}\u0000${t}`)) continue;
          addCandidate(from, name);
          addCandidate(t, name);
        }
      }
    };
    for (const c of enabledChannels) {
      for (const m of c.models ?? []) addCandidate(m, c.name);
      mappingHits(c.model_mapping, c.model_mapping_disabled, c.name);
    }
    for (const a of enabledAccounts) {
      for (const m of a.models ?? []) {
        if (m.status === "available") addCandidate(m.id, a.label);
      }
      mappingHits(a.model_mapping, a.model_mapping_disabled, a.label);
    }
    return map;
  }, [enabledChannels, enabledAccounts]);

  return (
    <div className="space-y-4">
      {/* 路由舞台 */}
      <section className="surface relative overflow-hidden rounded-[24px] p-6">
        <div className="mb-4 flex flex-wrap items-center justify-between gap-2">
          <h2 className="flex items-center gap-2 text-base font-semibold text-slate-900">
            <Radio size={16} className="text-blue-600" /> 网关路由
          </h2>
          <div className="flex items-center gap-2">
            <span className="inline-flex items-center gap-1.5 rounded-full border border-slate-200 bg-white px-2.5 py-1 text-[11px] text-slate-500">
              <Activity size={12} className="text-blue-500" />
              会话事件 {formatNumber(liveEvents)}
            </span>
            <button onClick={() => void load()} className="action-secondary" disabled={loading} style={{ padding: "5px 10px", fontSize: "12px" }}>
              <RefreshCw size={13} className={loading ? "animate-spin" : ""} /> 刷新
            </button>
          </div>
        </div>
        <div className="relative grid grid-cols-1 gap-6 md:grid-cols-[1fr_auto_1.4fr]">
          <SourceColumn keys={keys} recentModelByKey={recentModelByKey} />
          <HubColumn enabledCount={upstreamTotal} keyCount={keys.length} />
          <UpstreamColumn
            channels={enabledChannels}
            accounts={enabledAccounts}
            flowByChannel={flowByChannel}
            onToggleChannelMapping={(c, n, t, off) => void toggleChannelMapping(c, n, t, off)}
            onToggleAccountMapping={(a, n, t, off) => void toggleAccountMapping(a, n, t, off)}
          />
        </div>
        {/* 连线 + 粒子层常驻渲染：连线始终可见，无请求时播放空闲流光 */}
        <ParticleLayer
          particles={particles}
          hasUpstream={upstreamTotal > 0}
          sourceCount={keys.length}
          targetCount={upstreamTotal}
          modelCandidates={modelCandidates}
        />
      </section>

      {/* 渠道流量明细 */}
      <section className="surface rounded-[24px] p-6">
        <div className="flex items-center justify-between gap-2">
          <h2 className="flex items-center gap-2 text-base font-semibold text-slate-900">
            <Gauge size={16} className="text-blue-500" /> 渠道流量（近 24 小时）
          </h2>
          <Link to="/logs" className="text-xs text-blue-600 hover:underline">查看日志 →</Link>        </div>
        {flow.length === 0 ? (
          <p className="mt-4 text-sm text-slate-400">暂无流量数据；有请求经过网关后这里会显示各渠道的调用量与首字耗时。</p>
        ) : (
          <div className="mt-4 overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-slate-200 text-left text-xs text-slate-400">
                  <th className="pb-2 font-medium">渠道</th>
                  <th className="pb-2 text-right font-medium">请求数</th>
                  <th className="pb-2 text-right font-medium">失败</th>
                  <th className="pb-2 text-right font-medium">Token</th>
                  <th className="pb-2 text-right font-medium">平均首字</th>
                </tr>
              </thead>
              <tbody>
                {flow.map(row => (
                  <tr key={row.channel_id} className="border-b border-slate-100 last:border-0">
                    <td className="py-2.5 font-medium text-slate-700">{row.channel_name || row.channel_id}</td>
                    <td className="py-2.5 text-right tabular-nums text-slate-600">{formatNumber(row.request_count)}</td>
                    <td className={`py-2.5 text-right tabular-nums ${row.fail_count > 0 ? "text-red-500" : "text-slate-400"}`}>
                      {formatNumber(row.fail_count)}
                    </td>
                    <td className="py-2.5 text-right tabular-nums text-slate-600">{formatNumber(row.total_tokens)}</td>
                    <td className="py-2.5 text-right tabular-nums text-slate-600">
                      {row.avg_ttft_ms > 0 ? formatDuration(Math.round(row.avg_ttft_ms)) : "-"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}

// ─── 左栏：调用方（API Key）────────────────────────────────────────────────

function SourceColumn({ keys, recentModelByKey }: { keys: ApiKey[]; recentModelByKey: Map<string, string> }) {
  return (
    <div className="z-10 space-y-2.5">
      <div className="mb-3 flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wide text-slate-400">
        <Key size={12} /> 调用方
      </div>
      {keys.length === 0 && (
        <div className="rounded-2xl border border-dashed border-slate-200 p-4 text-xs text-slate-400">
          暂无密钥
        </div>
      )}
      {keys.slice(0, 8).map(k => {
        const recentModel = recentModelByKey.get(k.name);
        return (
          <div
            key={k.id}
            data-particle-source={k.name}
            className="group rounded-2xl border border-slate-200 bg-white px-4 py-3 shadow-[0_2px_8px_rgba(15,23,42,0.04)] transition-all hover:border-blue-200 hover:shadow-[0_4px_14px_rgba(47,111,237,0.1)]"
          >
            <div className="flex items-center gap-2">
              <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-lg bg-blue-50 text-blue-500">
                <Key size={12} />
              </span>
              <span className="min-w-0 flex-1 truncate text-sm font-medium text-slate-800" title={k.name}>{k.name}</span>
            </div>
            <div className="mt-1.5 flex items-center gap-1.5 pl-8 text-[10px] text-slate-400">
              <span className={`h-1.5 w-1.5 rounded-full ${k.status === 1 ? "bg-emerald-400" : "bg-slate-300"}`} />
              {k.status === 1 ? "启用" : "停用"}
              {(k.quota_limit ?? 0) > 0 && (
                <span className="ml-auto tabular-nums">{formatNumber(k.quota_used ?? 0)}/{formatNumber(k.quota_limit)}</span>
              )}
            </div>
            {recentModel && (
              <div className="mt-1 flex items-center gap-1 pl-8 text-[10px] text-blue-500" title={`最近调用模型：${recentModel}`}>
                <ArrowRight size={9} className="shrink-0" />
                <span className="truncate font-mono">{recentModel}</span>
              </div>
            )}
          </div>
        );
      })}
      {keys.length > 8 && (
        <Link to="/api-keys" className="block rounded-2xl border border-dashed border-slate-200 px-3.5 py-2 text-center text-xs text-slate-400 hover:text-slate-600">
          还有 {keys.length - 8} 个密钥 →
        </Link>
      )}
    </div>
  );
}

// ─── 中栏：网关 ──────────────────────────────────────────────────────────────

function HubColumn({ enabledCount, keyCount }: { enabledCount: number; keyCount: number }) {
  return (
    <div className="z-10 flex items-center justify-center md:w-48">
      <div
        data-hub
        className="route-map-hub-glow relative flex flex-col items-center gap-2 rounded-3xl border-2 border-blue-200 bg-gradient-to-b from-blue-50 to-white px-7 py-6"
      >
        <div className="relative flex h-14 w-14 items-center justify-center rounded-2xl bg-gradient-to-br from-blue-500 to-blue-700 shadow-lg">
          <Radio size={24} className="text-white" />
          <span className="absolute -right-1 -top-1 flex h-3 w-3">
            <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-blue-400 opacity-60" />
            <span className="relative inline-flex h-3 w-3 rounded-full bg-blue-500 ring-2 ring-white" />
          </span>
        </div>
        <div className="text-sm font-semibold tracking-tight text-slate-900">WaLiAPI</div>
        <div className="text-[10px] leading-4 text-slate-400">网关 · 路由/协议/审计</div>
        <div className="mt-1 flex gap-1.5 text-[10px] font-medium text-slate-600">
          <span className="rounded-full border border-slate-200 bg-white px-2 py-0.5 shadow-sm">{keyCount} 密钥</span>
          <span className="rounded-full border border-slate-200 bg-white px-2 py-0.5 shadow-sm">{enabledCount} 上游</span>
        </div>
      </div>
    </div>
  );
}

// ─── 右栏：上游渠道 + Auth 账号 ─────────────────────────────────────────────

export interface MappingPairChip {
  text: string;
  /** 是否处于关闭态（迁移 041/042 的 disabled pair）——灰色删除线展示。 */
  off: boolean;
}

const MAPPING_CHIPS_MAX = 4;

/** 渠道的映射对 chips（含被关闭的映射对，关闭的带 off 标记）。 */
function channelMappingPairs(channel: Channel): MappingPairChip[] | null {
  const mapping = channel.model_mapping;
  if (!mapping || typeof mapping !== "object") return null;
  const disabled = channel.model_mapping_disabled ?? [];
  const pairs: MappingPairChip[] = [];
  for (const [from, to] of Object.entries(mapping)) {
    const toList = Array.isArray(to) ? to : [to];
    for (const t of toList) {
      pairs.push({
        text: `${from} → ${t}`,
        off: disabled.some(d => d[0] === from && d[1] === t),
      });
    }
  }
  return pairs.length > 0 ? pairs.slice(0, MAPPING_CHIPS_MAX) : null;
}

/** Auth 账号的映射对 chips；无映射时回退 null（由调用方展示可用模型）。 */
function accountMappingPairs(account: AuthAccount): MappingPairChip[] | null {
  const mapping = account.model_mapping;
  if (!mapping || typeof mapping !== "object") return null;
  const disabled = account.model_mapping_disabled ?? [];
  const pairs: MappingPairChip[] = [];
  for (const [from, to] of Object.entries(mapping)) {
    const toList = Array.isArray(to) ? to : [to];
    for (const t of toList) {
      pairs.push({
        text: `${from} → ${t}`,
        off: disabled.some(d => d[0] === from && d[1] === t),
      });
    }
  }
  return pairs.length > 0 ? pairs.slice(0, MAPPING_CHIPS_MAX) : null;
}

/**
 * 右栏：两组上游节点。API 渠道与 Auth 账号（OAuth 订阅账号）分开展示，
 * 节点样式一致但徽标区分类型；Auth 账号展示配额窗口/模型映射摘要。
 * 映射 chip 支持点击电源快捷开关（乐观更新，失败回滚并静默提示）。
 */
function UpstreamColumn({
  channels,
  accounts,
  flowByChannel,
  onToggleChannelMapping,
  onToggleAccountMapping,
}: {
  channels: Channel[];
  accounts: AuthAccount[];
  flowByChannel: Map<string, RouteFlow>;
  onToggleChannelMapping: (channel: Channel, name: string, target: string, currentlyOff: boolean) => void;
  onToggleAccountMapping: (account: AuthAccount, name: string, target: string, currentlyOff: boolean) => void;
}) {
  return (
    <div className="z-10 space-y-2.5">
      <div className="mb-3 flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wide text-slate-400">
        <Boxes size={12} /> 上游 · 渠道 {channels.length} / Auth {accounts.length}
      </div>

      {/* API 渠道组 */}
      {channels.length === 0 && (
        <div className="rounded-2xl border border-dashed border-slate-200 p-3 text-xs text-slate-400">
          暂无启用渠道
        </div>
      )}
      {channels.slice(0, 8).map(c => {
        const probeFailed = c.last_probe_ok === 0;
        const row = flowByChannel.get(c.id);
        return (
          <UpstreamNode
            key={c.id}
            particleKey={c.name}
            title={c.name}
            badge={c.type}
            probeFailed={probeFailed}
            priority={c.priority}
            weight={c.weight}
            mappingPairs={channelMappingPairs(c)}
            onToggleMapping={(name, target, currentlyOff) => onToggleChannelMapping(c, name, target, currentlyOff)}
            models={c.models ?? []}
            flow={row}
          />
        );
      })}
      {channels.length > 8 && (
        <Link to="/channels" className="block rounded-2xl border border-dashed border-slate-200 px-3.5 py-2 text-center text-xs text-slate-400 hover:text-slate-600">
          还有 {channels.length - 8} 个渠道 →
        </Link>
      )}

      {/* Auth 账号组 */}
      {accounts.length > 0 && (
        <>
          <div className="flex items-center gap-2 pt-2">
            <span className="text-[10px] font-semibold uppercase tracking-wider text-violet-400">Auth 账号（订阅）</span>
            <span className="h-px flex-1 bg-gradient-to-r from-violet-200 to-transparent" />
          </div>
          {accounts.slice(0, 6).map(a => {
            const quotaPct = a.quota?.limits?.[0]?.primary?.used_percent ?? null;
            return (
              <UpstreamNode
                key={a.id}
                particleKey={a.label}
                title={a.label || a.email || a.provider}
                badge={a.provider}
                isAuth
                probeFailed={a.status !== "active" || Boolean(a.invalidation_reason)}
                subtitle={a.email && a.label && a.email !== a.label ? a.email : undefined}
                quotaPct={quotaPct}
                priority={a.priority}
                weight={a.weight}
                mappingPairs={accountMappingPairs(a)}
                onToggleMapping={(name, target, currentlyOff) => onToggleAccountMapping(a, name, target, currentlyOff)}
                models={(a.models ?? []).filter(m => m.status === "available").map(m => m.id)}
                flow={flowByChannel.get(a.id)}
              />
            );
          })}
          {accounts.length > 6 && (
            <Link to="/channels/auth" className="block rounded-2xl border border-dashed border-slate-200 px-3.5 py-2 text-center text-xs text-slate-400 hover:text-slate-600">
              还有 {accounts.length - 6} 个账号 →
            </Link>
          )}
        </>
      )}
    </div>
  );
}

/** 单个上游节点卡片（渠道与 Auth 账号共用样式）。 */
function UpstreamNode({
  particleKey,
  title,
  badge,
  isAuth = false,
  probeFailed,
  subtitle,
  quotaPct,
  priority,
  weight,
  mappingPairs,
  onToggleMapping,
  models,
  flow,
}: {
  particleKey: string;
  title: string;
  badge: string;
  /** Auth 账号节点用紫色系徽标区分。 */
  isAuth?: boolean;
  probeFailed: boolean;
  subtitle?: string;
  quotaPct?: number | null;
  /** 调度优先级（数字小者优先）。 */
  priority: number;
  /** 同优先级内的负载权重。 */
  weight: number;
  /** 映射对 chips（含关闭态：灰色删除线）。 */
  mappingPairs: MappingPairChip[] | null;
  /** 点击 chip 电源按钮的开关回调；缺省时 chip 无开关。 */
  onToggleMapping?: (name: string, target: string, currentlyOff: boolean) => void;
  /** 可用模型列表（映射 chips 不足时的补充展示）。 */
  models: string[];
  flow?: RouteFlow;
}) {
  return (
    <div
      data-particle-target={particleKey}
      className={`group rounded-2xl border px-4 py-3 shadow-[0_2px_8px_rgba(15,23,42,0.04)] transition-all hover:shadow-[0_4px_14px_rgba(47,111,237,0.1)] ${
        probeFailed
          ? "border-amber-200 bg-amber-50/60 hover:border-amber-300"
          : isAuth
            ? "border-violet-100 bg-white hover:border-violet-200"
            : "border-slate-200 bg-white hover:border-blue-200"
      }`}
    >
      <div className="flex items-center gap-2">
        <span className={`h-2 w-2 shrink-0 rounded-full ${probeFailed ? "bg-amber-400" : "bg-emerald-400"}`}>
          {!probeFailed && <span className="block h-full w-full animate-pulse rounded-full bg-emerald-400 opacity-60" />}
        </span>
        <span className="min-w-0 flex-1 truncate text-sm font-medium text-slate-800" title={title}>{title}</span>
        {/* 调度参数徽标：优先级 P（小者先）+ 权重 W（大者更常被选中） */}
        <span
          className="shrink-0 rounded-md bg-slate-50 px-1.5 py-0.5 text-[10px] font-medium tabular-nums text-slate-500"
          title={`调度优先级 ${priority}（数字小者优先） · 权重 ${weight}（同优先级内权重大者更常被选中）`}
        >
          P{priority} · W{weight}
        </span>
        {quotaPct != null && (
          <span
            className={`shrink-0 rounded-full px-1.5 py-0.5 text-[10px] font-medium tabular-nums ${
              quotaPct >= 90
                ? "bg-red-50 text-red-600"
                : quotaPct >= 70
                  ? "bg-amber-50 text-amber-600"
                  : "bg-slate-50 text-slate-500"
            }`}
            title="配额使用率"
          >
            {Math.round(quotaPct)}%
          </span>
        )}
        <span
          className={`shrink-0 rounded-md px-1.5 py-0.5 text-[10px] font-medium ${
            isAuth ? "bg-violet-100 text-violet-600" : "bg-slate-100 text-slate-500"
          }`}
        >
          {badge}
        </span>
      </div>
      {subtitle && (
        <div className="mt-0.5 truncate pl-4 text-[10px] text-slate-400" title={subtitle}>{subtitle}</div>
      )}
      {mappingPairs && mappingPairs.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1 pl-4">
          {mappingPairs.map(pair => {
            const [from, to] = pair.text.split(" → ");
            const clickable = Boolean(onToggleMapping) && from != null && to != null;
            return (
              <span
                key={pair.text}
                className={`group/chip inline-flex items-center gap-0.5 rounded-full py-0 pl-1.5 pr-1 text-[10px] font-medium leading-4 font-mono transition-colors ${
                  pair.off
                    ? "bg-slate-100 text-slate-400"
                    : "bg-violet-50 text-violet-600"
                }`}
                title={pair.off ? `映射已关闭（${pair.text}），点击电源开启` : `映射 ${pair.text}，点击电源关闭`}
              >
                <span className={pair.off ? "line-through decoration-slate-300" : ""}>{pair.text}</span>
                {pair.off && <span className="font-sans text-[9px] text-slate-400">已关闭</span>}
                {clickable && (
                  <button
                    onClick={() => onToggleMapping!(from, to, pair.off)}
                    className={`rounded-full p-0.5 transition-colors ${
                      pair.off
                        ? "text-slate-400 hover:bg-slate-200 hover:text-emerald-600"
                        : "text-violet-400 hover:bg-violet-100 hover:text-red-500"
                    }`}
                    aria-label={pair.off ? `开启映射 ${pair.text}` : `关闭映射 ${pair.text}`}
                  >
                    <Power size={9} />
                  </button>
                )}
              </span>
            );
          })}
        </div>
      )}
      {/* 无映射时展示可用模型（最多 3 个）；有映射时若还有空间也补充展示直连模型 */}
      {models.length > 0 && (!mappingPairs || mappingPairs.length < MAPPING_CHIPS_MAX) && (
        <div className="mt-1.5 flex flex-wrap gap-1 pl-4">
          {models.slice(0, 3).map(m => (
            <span
              key={m}
              className="inline-flex items-center rounded-full bg-slate-100 px-1.5 py-0 text-[10px] font-medium leading-4 text-slate-500"
              title={`可用模型 ${m}`}
            >
              {m}
            </span>
          ))}
        </div>
      )}
      {flow && (
        <div className="mt-1 flex gap-3 pl-4 text-[10px] tabular-nums text-slate-400">
          <span>{formatNumber(flow.request_count)} 请求</span>
          {flow.avg_ttft_ms > 0 && <span>首字 {formatDuration(Math.round(flow.avg_ttft_ms))}</span>}
        </div>
      )}
    </div>
  );
}

// ─── 连线层 + 粒子层（常驻渲染）─────────────────────────────────────────────

/**
 * 三次贝塞尔曲线上 t 处的点（同步纯函数）：
 * 之前用隐藏 path + getPointAtLength + setState 异步求点，粒子 700ms 生命周期
 * 内常常一帧都没画出来（连线也因此「看不到动态」）。改为纯数学插值后
 * 每帧同步定位，配合 rAF 连续重绘。
 */
function cubicPointAt(
  from: { x: number; y: number },
  to: { x: number; y: number },
  t: number,
): { x: number; y: number } {
  const dx = Math.max(Math.abs(to.x - from.x) * 0.45, 40);
  const c1 = { x: from.x + dx, y: from.y };
  const c2 = { x: to.x - dx, y: to.y };
  const u = 1 - t;
  return {
    x: u * u * u * from.x + 3 * u * u * t * c1.x + 3 * u * t * t * c2.x + t * t * t * to.x,
    y: u * u * u * from.y + 3 * u * u * t * c1.y + 3 * u * t * t * c2.y + t * t * t * to.y,
  };
}

/**
 * 粒子目标端点：仅按渠道名精确匹配，找不到返回 undefined。
 * 禁止兜底到第一个渠道——那会把无关渠道的连线点亮，产生「路由错了」的错觉。
 */
function fallbackTargetInfo(
  channelStarts: Map<string, { x: number; y: number }>,
  name: string | null,
): { x: number; y: number } | undefined {
  if (name && channelStarts.has(name)) return channelStarts.get(name);
  return undefined;
}

function ParticleLayer({
  particles,
  hasUpstream,
  sourceCount,
  targetCount,
  modelCandidates,
}: {
  particles: Particle[];
  hasUpstream: boolean;
  sourceCount: number;
  targetCount: number;
  /** 模型名 → 可能承接的渠道/Auth 名集合（请求未路由前的候选提示用）。 */
  modelCandidates: Map<string, Set<string>>;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [geometry, setGeometry] = useState<{
    keyEnds: Map<string, { x: number; y: number }>;
    channelStarts: Map<string, { x: number; y: number }>;
    hubLeft: { x: number; y: number };
    hubRight: { x: number; y: number };
    width: number;
    height: number;
  }>({
    keyEnds: new Map(),
    channelStarts: new Map(),
    hubLeft: { x: 0, y: 0 },
    hubRight: { x: 0, y: 0 },
    width: 0,
    height: 0,
  });

  // 端点计算：源卡片右缘中心 → hub 左缘中心；hub 右缘中心 → 目标卡片左缘中心。
  // 关键：查询与坐标参照都必须用 section 根节点——覆盖层自身是空的绝对定位
  // div（只含 SVG），data-hub / 节点卡片是它的兄弟节点，从 container 里查永远
  // 查不到（历史 bug：几何恒为 0×0，viewBox "0 0 0 0"，连线/粒子从未渲染过）。
  // 覆盖层 inset-0 与 section 完全重合，因此以 section rect 为原点即为覆盖层坐标。
  useEffect(() => {
    const compute = () => {
      const container = containerRef.current;
      const root = container?.parentElement;
      if (!container || !root) return;
      const rect = root.getBoundingClientRect();
      const hub = root.querySelector("[data-hub]")?.getBoundingClientRect();
      if (!hub) return;
      const hubY = hub.top + hub.height / 2 - rect.top;
      const keyEnds = new Map<string, { x: number; y: number }>();
      root.querySelectorAll<HTMLElement>("[data-particle-source]").forEach(el => {
        const r = el.getBoundingClientRect();
        keyEnds.set(el.dataset.particleSource || "", {
          x: r.right - rect.left,
          y: r.top + r.height / 2 - rect.top,
        });
      });
      const channelStarts = new Map<string, { x: number; y: number }>();
      root.querySelectorAll<HTMLElement>("[data-particle-target]").forEach(el => {
        const r = el.getBoundingClientRect();
        channelStarts.set(el.dataset.particleTarget || "", {
          x: r.left - rect.left,
          y: r.top + r.height / 2 - rect.top,
        });
      });
      setGeometry({
        keyEnds,
        channelStarts,
        hubLeft: { x: hub.left - rect.left, y: hubY },
        hubRight: { x: hub.right - rect.left, y: hubY },
        width: rect.width,
        height: rect.height,
      });
    };
    compute();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(compute);
    // 观察 section 根节点：右栏渠道异步加载后 section 高度变化会触发重算
    if (containerRef.current?.parentElement) ro.observe(containerRef.current.parentElement);
    return () => ro.disconnect();
  }, [hasUpstream, sourceCount, targetCount]);

  // 空闲流光动画：无粒子时连线上也有缓慢流动的光斑（rAF 全局时钟，~3s 周期）。
  const idleFlow = particles.length === 0 && hasUpstream;

  // 高亮路径：最近 1 条活跃请求（含 done 后的 LINGER 停留期）覆盖的
  // Key→网关→渠道 两段连线。只有一条线被点亮，调用路径一眼可辨。
  // （多路并发时显示最新一条——叠加多条高亮会糊成一片，反而看不清。）
  const now = Date.now();
  const active = particles.length > 0 ? particles[particles.length - 1] : null;
  const highlightKey = active?.keyName ?? null;
  const highlightTarget = active
    ? fallbackTargetInfo(geometry.channelStarts, active.channelName)
    : undefined;
  const highlightColor = active?.failed ? "#f87171" : "#3b82f6";

  return (
    <div ref={containerRef} className="pointer-events-none absolute inset-0 z-0">
      <svg className="absolute inset-0 h-full w-full" viewBox={`0 0 ${geometry.width} ${geometry.height}`}>
        <defs>
          <linearGradient id="wire-grad" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stopColor="#94a3b8" stopOpacity="0.55" />
            <stop offset="50%" stopColor="#64748b" stopOpacity="0.4" />
            <stop offset="100%" stopColor="#94a3b8" stopOpacity="0.55" />
          </linearGradient>
        </defs>

        {/* 静态连线：Key → hub 与 hub → 目标，常驻可见（中性灰、加粗，浅色背景上不再隐形） */}
        {[...geometry.keyEnds.entries()].map(([name, end]) => (
          <WirePath key={`k-${name}`} from={end} to={geometry.hubLeft} />
        ))}
        {[...geometry.channelStarts.entries()].map(([name, start]) => (
          <WirePath key={`c-${name}`} from={geometry.hubRight} to={start} />
        ))}

        {/* 高亮路径层：调用发生时对应连线加粗点亮 + 虚线快速流动 */}
        {active && (() => {
          const keyEnd = geometry.keyEnds.get(highlightKey || "") ?? [...geometry.keyEnds.values()][0];
          const target = highlightTarget;
          const inboundAge = now - active.startedAt;
          // done 后粒子已消失但路径仍在 LINGER 停留期内 → 持续点亮
          const lit = inboundAge < INFLIGHT_MS + OUTBOUND_MS + DONE_MS + LINGER_MS;
          if (!lit) return null;
          // 尚未拿到确切渠道名（start 阶段）：按模型反查点亮所有候选渠道的
          // 淡色虚线，提示「可能走这几条」；done 后只留实际那条亮线。
          const candidateNames =
            target || !active.modelName
              ? []
              : [...(modelCandidates.get(active.modelName.trim().toLowerCase()) ?? [])];
          return (
            <g>
              {keyEnd && (
                <WirePath
                  from={keyEnd}
                  to={geometry.hubLeft}
                  variant="active"
                  color={highlightColor}
                />
              )}
              {target && (
                <WirePath
                  from={geometry.hubRight}
                  to={target}
                  variant="active"
                  color={highlightColor}
                />
              )}
              {!target &&
                candidateNames.map(name => {
                  const start = geometry.channelStarts.get(name);
                  return start ? (
                    <WirePath
                      key={`cand-${name}`}
                      from={geometry.hubRight}
                      to={start}
                      variant="candidate"
                    />
                  ) : null;
                })}
            </g>
          );
        })()}

        {/* 空闲流光 */}
        {idleFlow && <IdleFlowDots geometry={geometry} />}

        {/* 请求粒子。outbound/done 仅在渠道名精确匹配时渲染——
            start 事件的粒子没有渠道信息，只进入站段，不猜线。 */}
        {particles.map(p => {
          const keyEnd = geometry.keyEnds.get(p.keyName) ?? [...geometry.keyEnds.values()][0];
          if (!keyEnd) return null;
          const target = fallbackTargetInfo(geometry.channelStarts, p.channelName);

          if (p.phase === "inflight") {
            const t = easeInOut((Date.now() - p.startedAt) / INFLIGHT_MS);
            return <FlyingDot key={p.id} from={keyEnd} to={geometry.hubLeft} t={t} failed={false} inbound />;
          }
          if (p.phase === "outbound" && target) {
            const t = easeInOut((Date.now() - p.startedAt - INFLIGHT_MS) / OUTBOUND_MS);
            return <FlyingDot key={p.id} from={geometry.hubRight} to={target} t={t} failed={p.failed} />;
          }
          if (p.phase === "done" && target) {
            return <EndPulse key={p.id} at={target} failed={p.failed} />;
          }
          return null;
        })}
      </svg>
    </div>
  );
}

/** 缓入缓出，粒子运动更有弹性。 */
function easeInOut(t: number): number {
  const clamped = Math.min(Math.max(t, 0), 1);
  return clamped < 0.5 ? 2 * clamped * clamped : 1 - Math.pow(-2 * clamped + 2, 2) / 2;
}

/**
 * 连线（贝塞尔 + 虚线）。
 * idle：中性灰细虚线——铺底可见但不抢戏；
 * candidate：淡蓝中虚线、缓慢流动——请求未路由前按模型反查的「可能走这条」；
 * active：亮色（蓝/红）加粗虚线、快速流动——调用确定后点亮实际路径。
 */
function WirePath({
  from,
  to,
  variant = "idle",
  color = "#3b82f6",
}: {
  from: { x: number; y: number };
  to: { x: number; y: number };
  variant?: "idle" | "active" | "candidate";
  color?: string;
}) {
  const dx = Math.max(Math.abs(to.x - from.x) * 0.45, 40);
  const d = `M ${from.x} ${from.y} C ${from.x + dx} ${from.y}, ${to.x - dx} ${to.y}, ${to.x} ${to.y}`;
  if (variant === "candidate") {
    return (
      <path
        d={d}
        fill="none"
        stroke="#60a5fa"
        strokeWidth="2"
        strokeDasharray="4 6"
        strokeLinecap="round"
        opacity={0.5}
        className="route-map-wire"
      />
    );
  }
  const isActive = variant === "active";
  return (
    <path
      d={d}
      fill="none"
      stroke={isActive ? color : "url(#wire-grad)"}
      strokeWidth={isActive ? 2.5 : 1.5}
      strokeDasharray={isActive ? "7 5" : "5 6"}
      strokeLinecap="round"
      opacity={isActive ? 0.95 : 0.8}
      className={isActive ? "route-map-wire-active" : "route-map-wire"}
    />
  );
}

/** 飞行粒子：光点 + 外圈光晕 + 拖尾（拖尾取 t 略早的几个采样点，透明度递减）。 */
function FlyingDot({
  from,
  to,
  t,
  failed,
  inbound = false,
}: {
  from: { x: number; y: number };
  to: { x: number; y: number };
  t: number;
  failed: boolean;
  inbound?: boolean;
}) {
  const clamped = Math.min(Math.max(t, 0), 1);
  const head = cubicPointAt(from, to, clamped);
  const trail = [0.06, 0.12, 0.19].map((back, i) => ({
    p: cubicPointAt(from, to, Math.max(clamped - back, 0)),
    o: [0.5, 0.3, 0.15][i],
  }));
  const color = failed ? "#f87171" : inbound ? "#60a5fa" : "#3b82f6";
  return (
    <g>
      {trail.map((s, i) => (
        <circle key={i} cx={s.p.x} cy={s.p.y} r={2.6 - i * 0.5} fill={color} opacity={s.o} />
      ))}
      <circle cx={head.x} cy={head.y} r={5.5} fill={color} opacity={0.25} />
      <circle cx={head.x} cy={head.y} r={3.2} fill={color} stroke="white" strokeWidth="1.2" />
    </g>
  );
}

/** 完成脉冲：终点双环扩散一次（CSS animate）。 */
function EndPulse({ at, failed }: { at: { x: number; y: number }; failed: boolean }) {
  const color = failed ? "#f87171" : "#34d399";
  return (
    <g>
      <circle cx={at.x} cy={at.y} r={3} fill={color} stroke="white" strokeWidth="1.2" />
      <circle cx={at.x} cy={at.y} r={6} fill="none" stroke={color} strokeWidth="1.5" className="route-map-pulse" />
    </g>
  );
}

/** 空闲流光：每条连线上一个沿曲线慢速移动的小光点（rAF 全局时钟驱动）。 */
function IdleFlowDots({
  geometry,
}: {
  geometry: {
    keyEnds: Map<string, { x: number; y: number }>;
    channelStarts: Map<string, { x: number; y: number }>;
    hubLeft: { x: number; y: number };
    hubRight: { x: number; y: number };
    width: number;
    height: number;
  };
}) {
  const [clock, setClock] = useState(0);
  useEffect(() => {
    let raf = 0;
    const loop = () => {
      setClock(c => (c + 1) % 100000);
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);
  const t = (clock % 180) / 180; // ~3s 一个来回周期
  const pingpong = t < 0.5 ? t * 2 : 2 - t * 2;
  return (
    <g>
      {[...geometry.keyEnds.values()].map((end, i) => {
        const p = cubicPointAt(end, geometry.hubLeft, (pingpong + i * 0.37) % 1);
        return <circle key={`f${i}`} cx={p.x} cy={p.y} r={2} fill="#93c5fd" opacity={0.55} />;
      })}
      {[...geometry.channelStarts.values()].map((start, i) => {
        const p = cubicPointAt(geometry.hubRight, start, (pingpong + i * 0.29) % 1);
        return <circle key={`g${i}`} cx={p.x} cy={p.y} r={2} fill="#93c5fd" opacity={0.55} />;
      })}
    </g>
  );
}
