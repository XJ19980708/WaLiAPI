import { Fragment, useEffect, useMemo, useState } from "react";
import { statsApi } from "../lib/api";
import type { DailyTokenPoint } from "../types";
import { formatNumber } from "../lib/constants";
import { Flame } from "lucide-react";

/**
 * Token 消耗热力图（GitHub contributions 风格）。
 *
 * 固定展示近一年多（54 周），拆成上下两半（各 27 周）纵向排布，两半列数
 * 相同、左右对齐，格子宽度自适应铺满卡片。每格代表一天，颜色深浅 = 当日
 * token 消耗的分位档。交互：hover 显示日期/token/请求明细。
 * 数据读 usage_stats 按天聚合。
 */

// 上下两半各 27 周，合计 54 周 ≈ 一年多
const TOTAL_WEEKS = 54;
const HALF_WEEKS = TOTAL_WEEKS / 2;

// 分位档配色（emerald 系，与仪表盘缓存命中色一致；0 档为空底）。
const LEVEL_CLASSES = [
  "bg-slate-100",           // 无数据
  "bg-emerald-200",         // L1
  "bg-emerald-300",
  "bg-emerald-400",
  "bg-emerald-500",
  "bg-emerald-600",         // L5 最高
];
const WEEK_LABELS = ["一", "二", "三", "四", "五", "六", "日"];
const MONTH_LABELS = ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"];

interface WeekCell {
  day: string;        // YYYY-MM-DD
  tokens: number;
  requests: number;
  level: number;      // 0-5
  future: boolean;    // 未来日期置灰
}

interface HalfData {
  weeks: WeekCell[][];
  marks: Array<{ col: number; label: string }>;
}

export function TokenHeatmap() {
  const [data, setData] = useState<DailyTokenPoint[]>([]);
  const [hover, setHover] = useState<WeekCell | null>(null);

  useEffect(() => {
    const doLoad = () => statsApi.getDailyTokenStats(TOTAL_WEEKS * 7 + 7).then(setData).catch(() => {});
    doLoad();
    const interval = setInterval(doLoad, 60000);
    return () => clearInterval(interval);
  }, []);

  const byDay = useMemo(() => {
    const map = new Map<string, DailyTokenPoint>();
    for (const row of data) map.set(row.day, row);
    return map;
  }, [data]);

  // 分位阈值：非零天数四分位切 5 档，保证颜色分布均匀（GitHub 同款思路）。
  const thresholds = useMemo(() => {
    const nonZero = data.map(d => d.total_tokens).filter(t => t > 0).sort((a, b) => a - b);
    if (nonZero.length === 0) return [1, 2, 3, 4, 5];
    const q = (p: number) => nonZero[Math.min(nonZero.length - 1, Math.floor(nonZero.length * p))];
    return [q(0.2), q(0.4), q(0.6), q(0.8), nonZero[nonZero.length - 1]];
  }, [data]);

  const { halves, totalTokens, totalRequests, activeDays } = useMemo(() => {
    const today = new Date();
    const todayStr = toDateStr(today);
    // 网格共 TOTAL_WEEKS 列：从 (TOTAL_WEEKS-1) 周前的周一开始，
    // 最后一列为当前周（今天之后的日期由 future 置灰）。
    const start = new Date(today);
    // 对齐到本周一（getDay: 0=周日）
    start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
    start.setDate(start.getDate() - (TOTAL_WEEKS - 1) * 7);

    const weeks: WeekCell[][] = [];
    const cursor = new Date(start);
    let totalTokens = 0;
    let totalRequests = 0;
    let activeDays = 0;
    for (let col = 0; col < TOTAL_WEEKS * 7; col++) {
      const dayStr = toDateStr(cursor);
      const row = byDay.get(dayStr);
      const tokens = row?.total_tokens ?? 0;
      const requests = row?.request_count ?? 0;
      const future = dayStr > todayStr;
      const level = future || tokens === 0 ? 0 : 1 + thresholds.findIndex(th => tokens <= th);
      const weekIdx = Math.floor(col / 7);
      if (!weeks[weekIdx]) weeks[weekIdx] = [];
      weeks[weekIdx].push({ day: dayStr, tokens, requests, level: Math.min(level, 5), future });
      if (!future) {
        totalTokens += tokens;
        totalRequests += requests;
        if (tokens > 0) activeDays += 1;
      }
      cursor.setDate(cursor.getDate() + 1);
    }

    // 上下两半，各自独立计算月份标记（每列首个不同月份的周一）
    const halves: HalfData[] = [weeks.slice(0, HALF_WEEKS), weeks.slice(HALF_WEEKS)].map(cells => {
      const marks: Array<{ col: number; label: string }> = [];
      let lastMonth = -1;
      cells.forEach((week, c) => {
        if (!week[0]) return;
        const m = Number(week[0].day.slice(5, 7)) - 1;
        if (m !== lastMonth) {
          marks.push({ col: c, label: MONTH_LABELS[m] });
          lastMonth = m;
        }
      });
      return { weeks: cells, marks };
    });

    return { halves, totalTokens, totalRequests, activeDays };
  }, [data, byDay, thresholds]);

  return (
    <section className="surface flex h-full flex-col rounded-[24px] p-6">
      {/* 头部：标题 + 汇总 */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="flex items-center gap-2 text-base font-semibold text-slate-900">
          <Flame size={16} className="text-emerald-500" /> Token 消耗热力图
        </h2>
      </div>
      <div className="mt-1 text-xs text-slate-500">
        近一年（{TOTAL_WEEKS} 周）共 <span className="font-semibold tabular-nums text-slate-700">{formatNumber(totalTokens)}</span> tokens ·{" "}
        <span className="tabular-nums">{formatNumber(totalRequests)}</span> 次请求 ·{" "}
        <span className="tabular-nums">{activeDays}</span> 天活跃
      </div>

      {/* 上下两半网格：各 27 周，列对齐、铺满宽度 */}
      <div className="mt-4 flex flex-1 flex-col justify-center gap-4">
        {halves.map((half, i) => (
          <HalfGrid key={i} half={half} onHover={setHover} />
        ))}
      </div>

      {/* 底部：hover 明细（固定高度防跳动）+ 图例 */}
      <div className="mt-4 flex min-h-[20px] flex-wrap items-center justify-between gap-3">
        <div className="text-xs text-slate-500">
          {hover ? (
            <>
              <span className="font-medium text-slate-700">{formatDay(hover.day)}</span>
              <span className="mx-2 text-slate-300">·</span>
              <span className="tabular-nums">{formatNumber(hover.tokens)} tokens</span>
              <span className="mx-2 text-slate-300">·</span>
              <span className="tabular-nums">{formatNumber(hover.requests)} 次请求</span>
            </>
          ) : (
            <span className="text-slate-400">悬停方块查看当日明细</span>
          )}
        </div>
        <div className="flex items-center gap-1.5 text-[10px] text-slate-400">
          少
          {LEVEL_CLASSES.map((cls, i) => (
            <span key={i} className={`h-[11px] w-[11px] rounded-[3px] ${cls}`} />
          ))}
          多
        </div>
      </div>
    </section>
  );
}

/** 半年区块：月份标记行 + 7 行（周一～周日）× N 列（周），格子自适应铺满。 */
function HalfGrid({ half, onHover }: { half: HalfData; onHover: (cell: WeekCell | null) => void }) {
  return (
    <div
      className="grid gap-[3px]"
      style={{ gridTemplateColumns: `1.75rem repeat(${half.weeks.length}, minmax(0, 1fr))` }}
    >
      {/* 月份标记行（首列为星期标签占位） */}
      <div />
      {half.weeks.map((_, c) => {
        const mark = half.marks.find(m => m.col === c);
        return (
          <div key={`m${c}`} className="whitespace-nowrap text-[10px] leading-none text-slate-400">
            {mark ? mark.label : ""}
          </div>
        );
      })}
      {/* 7 行网格：行标签 + 每周的 7 格 */}
      {WEEK_LABELS.map((label, row) => (
        <Fragment key={row}>
          <div className="flex items-center text-[10px] leading-none text-slate-400">
            {row % 2 === 0 ? label : ""}
          </div>
          {half.weeks.map((week, c) => {
            const cell = week[row];
            if (!cell) return <div key={c} className="aspect-square w-full" />;
            return (
              <div
                key={c}
                title={`${formatDay(cell.day)} · ${formatNumber(cell.tokens)} tokens`}
                className={`aspect-square w-full rounded-[3px] transition-transform hover:scale-125 hover:ring-1 hover:ring-slate-400 ${
                  cell.future ? "bg-slate-50" : LEVEL_CLASSES[cell.level]
                }`}
                onMouseEnter={() => onHover(cell)}
                onMouseLeave={() => onHover(null)}
              />
            );
          })}
        </Fragment>
      ))}
    </div>
  );
}

function toDateStr(d: Date): string {
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function formatDay(day: string): string {
  const [y, m, d] = day.split("-").map(Number);
  return `${y}/${m}/${d}`;
}
