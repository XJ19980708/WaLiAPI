import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { Zap, GitBranch, Image } from "lucide-react";
import { logApi } from "../../lib/api";
import type { LogPromptPart, LogAttempt } from "../../types";
import { formatDuration, formatNumber } from "../../lib/constants";

// ─── 分类展示元数据（与后端 prompt_classify.rs 的 KIND_* 一一对应）──────────

export const PROMPT_PART_META: Record<
  string,
  { label: string; color: string; bar: string; bg: string }
> = {
  system:      { label: "系统提示词", color: "text-slate-600",    bar: "bg-slate-400",   bg: "bg-slate-50" },
  tools:       { label: "工具定义",   color: "text-purple-600",  bar: "bg-purple-400",  bg: "bg-purple-50" },
  injected:    { label: "注入内容",   color: "text-amber-600",   bar: "bg-amber-400",   bg: "bg-amber-50" },
  assistant:   { label: "助手消息",   color: "text-emerald-600", bar: "bg-emerald-400", bg: "bg-emerald-50" },
  tool_result: { label: "工具结果",   color: "text-blue-600",    bar: "bg-blue-400",    bg: "bg-blue-50" },
  chat:        { label: "用户消息",   color: "text-sky-600",     bar: "bg-sky-400",     bg: "bg-sky-50" },
};

const PART_ORDER = ["system", "tools", "injected", "assistant", "tool_result", "chat"];

function partMeta(kind: string) {
  return PROMPT_PART_META[kind] || { label: kind, color: "text-slate-500", bar: "bg-slate-300", bg: "bg-slate-50" };
}

// ─── 上下文构成（分类 token 数据） ──────────────────────────────────────────

/** 加载单条日志的分类 token 拆分；数据未就绪/无数据时 children 原样渲染。 */
export function PromptPartsData({
  logId,
  children,
}: {
  logId: string;
  children: (parts: LogPromptPart[], calibrated: boolean) => ReactNode;
}) {
  const [parts, setParts] = useState<LogPromptPart[] | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    logApi.getPromptParts(logId)
      .then(rows => { if (!cancelled) setParts(rows); })
      .catch(() => { if (!cancelled) setError(true); });
    return () => { cancelled = true; };
  }, [logId]);

  if (error || !parts) return children([], false);

  const nonZero = parts
    .filter(p => p.tokens > 0)
    .sort((a, b) => {
      const ia = PART_ORDER.indexOf(a.kind);
      const ib = PART_ORDER.indexOf(b.kind);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
    });
  if (nonZero.length === 0) return children([], false);

  const calibrated = nonZero.some(p => p.calibrated);
  return children(nonZero, calibrated);
}

/**
 * 「对话构成」徽标行内嵌的分类 token 摘要（迁移 048）。按比例条 + 明细徽标渲染：
 * 每个分类一枚徽标，内含色点、名称、条数、token 数与占比；未校准时 token 前带 ≈。
 */
export function PromptPartsBadges({
  parts,
  calibrated,
  hasImage,
}: {
  parts: LogPromptPart[];
  calibrated: boolean;
  hasImage: boolean;
}) {
  if (parts.length === 0) {
    return hasImage ? (
      <span className="inline-flex items-center gap-1 rounded-md bg-purple-50 border border-purple-200 px-2 py-0.5 text-[11px] font-medium text-purple-700">
        <Image size={11} /> 含图片
      </span>
    ) : null;
  }

  const total = parts.reduce((sum, p) => sum + p.tokens, 0);

  return (
    <>
      {/* 比例条（与徽标同一行内，占满剩余宽度） */}
      <div className="flex h-2 w-16 min-w-16 self-center overflow-hidden rounded-full bg-slate-100 md:w-24" title="上下文构成比例">
        {parts.map(p => (
          <div
            key={p.kind}
            className={partMeta(p.kind).bar}
            style={{ width: `${(p.tokens / total) * 100}%` }}
            title={`${partMeta(p.kind).label}: ${formatNumber(p.tokens)} (${Math.round((p.tokens / total) * 100)}%)`}
          />
        ))}
      </div>
      {/* 分类明细徽标 */}
      {parts.map(p => {
        const meta = partMeta(p.kind);
        const pct = total > 0 ? Math.round((p.tokens / total) * 100) : 0;
        return (
          <span
            key={p.kind}
            className={`inline-flex items-center gap-1 rounded-md bg-slate-50 border border-slate-200 px-2 py-0.5 text-[11px] font-medium ${meta.color}`}
            title={`${meta.label}: ${calibrated ? "" : "≈"}${formatNumber(p.tokens)} tokens${pct > 0 ? ` (${pct}%)` : ""}`}
          >
            <span className={`h-2 w-2 shrink-0 rounded-sm ${meta.bar}`} />
            <span className="shrink-0">{meta.label}</span>
            {p.items > 0 && <span className="shrink-0 font-normal text-slate-400">×{p.items}</span>}
            <span className="shrink-0 font-normal text-slate-500 tabular-nums">
              {calibrated ? "" : "≈"}{formatNumber(p.tokens)}
            </span>
            <span className="shrink-0 font-normal text-slate-400 tabular-nums">{pct}%</span>
          </span>
        );
      })}
      {hasImage && (
        <span className="inline-flex items-center gap-1 rounded-md bg-purple-50 border border-purple-200 px-2 py-0.5 text-[11px] font-medium text-purple-700">
          <Image size={11} /> 含图片
        </span>
      )}
    </>
  );
}

// ─── 尝试时间线卡片 ──────────────────────────────────────────────────────────

const OUTCOME_META: Record<string, { label: string; cls: string }> = {
  success: { label: "成功", cls: "bg-emerald-50 text-emerald-700 border-emerald-200" },
  rate_limited: { label: "限流", cls: "bg-amber-50 text-amber-700 border-amber-200" },
  quota_exceeded: { label: "额度用尽", cls: "bg-orange-50 text-orange-700 border-orange-200" },
  upstream_timeout: { label: "上游超时", cls: "bg-red-50 text-red-700 border-red-200" },
  upstream_http_5xx: { label: "上游 5xx", cls: "bg-red-50 text-red-700 border-red-200" },
  authentication_failed: { label: "鉴权失败", cls: "bg-rose-50 text-rose-700 border-rose-200" },
  protocol_failed: { label: "协议错误", cls: "bg-violet-50 text-violet-700 border-violet-200" },
  request_rejected: { label: "请求被拒", cls: "bg-slate-100 text-slate-600 border-slate-200" },
  client_cancelled: { label: "客户端取消", cls: "bg-slate-100 text-slate-500 border-slate-200" },
};

function outcomeMeta(outcome: string | null) {
  return (
    (outcome && OUTCOME_META[outcome]) || {
      label: outcome || "未知",
      cls: "bg-slate-100 text-slate-500 border-slate-200",
    }
  );
}

/**
 * 上游物理尝试明细（迁移 048）：多渠道 failover 时逐条展示每次尝试的
 * 渠道、结果与耗时；单次成功尝试只显示一行摘要。
 */
export function AttemptsCard({ logId }: { logId: string }) {
  const [attempts, setAttempts] = useState<LogAttempt[] | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    logApi.getAttempts(logId)
      .then(rows => { if (!cancelled) setAttempts(rows); })
      .catch(() => { if (!cancelled) setError(true); });
    return () => { cancelled = true; };
  }, [logId]);

  if (error || !attempts || attempts.length === 0) return null;
  // 单次成功、无失败尝试时不需要时间线（列表行已表达完整）。
  if (attempts.length <= 1 && attempts[0]?.is_final) return null;

  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3">
      <div className="flex items-center gap-1.5 text-xs font-medium text-slate-600">
        <GitBranch size={13} /> 调度尝试（{attempts.length} 次）
      </div>
      <div className="mt-2.5 space-y-1.5">
        {attempts.map(a => {
          const meta = outcomeMeta(a.outcome);
          return (
            <div key={a.seq} className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px]">
              <span className="w-10 shrink-0 font-mono text-slate-400">#{a.seq}</span>
              <span className="min-w-0 max-w-[180px] truncate font-medium text-slate-700" title={a.channel_name || "-"}>
                {a.channel_name || "-"}
              </span>
              {a.upstream_model && a.upstream_model !== a.channel_name && (
                <span className="min-w-0 max-w-[140px] truncate font-mono text-[10px] text-slate-400" title={a.upstream_model}>
                  {a.upstream_model}
                </span>
              )}
              <span className={`shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] font-medium ${meta.cls}`}>
                {meta.label}{a.status_code != null ? ` ${a.status_code}` : ""}
              </span>
              {a.ttft_ms != null && (
                <span className="inline-flex shrink-0 items-center gap-0.5 text-slate-500" title="首字耗时">
                  <Zap size={10} /> {formatDuration(a.ttft_ms)}
                </span>
              )}
              {a.duration_ms != null && (
                <span className="ml-auto shrink-0 tabular-nums text-slate-400">
                  {formatDuration(a.duration_ms)}
                </span>
              )}
              {a.error_message && (
                <div className="w-full min-w-0 truncate text-[10px] text-slate-400" title={a.error_message}>
                  {a.error_message}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ─── 首字耗时徽标（列表行用）────────────────────────────────────────────────

/** 列表「耗时」列上方的首字耗时小字；无数据返回 null。 */
export function TtftLabel({ ttftMs }: { ttftMs?: number | null }) {
  if (ttftMs == null || ttftMs <= 0) return null;
  return (
    <span
      className="inline-flex items-center gap-0.5 text-[10px] text-slate-400 whitespace-nowrap"
      title="首字耗时（TTFT）"
    >
      <Zap size={9} className="text-amber-400" /> {formatDuration(ttftMs)}
    </span>
  );
}
