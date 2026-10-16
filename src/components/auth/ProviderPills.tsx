import { useEffect, useState } from "react";
import { Command } from "lucide-react";
import { authApi } from "../../lib/api";
import type { AuthProviderInfo } from "../../types";

const iconFor = (key: string): string => {
  if (key === "codex") return "⌘";
  if (key === "moonshot") return "☾";
  if (key === "google") return "G";
  if (key === "grok") return "✦";
  if (key === "workbuddy") return "W";
  if (key === "qoder") return "Q";
  return "◎";
};

export function ProviderPills({
  selected,
  onSelect,
  counts,
}: {
  selected: string | null;
  onSelect: (providerId: string) => void;
  /** 各 provider 下的账号数量（由页面基于全量账号列表统计传入） */
  counts?: Record<string, number>;
}) {
  const [providers, setProviders] = useState<AuthProviderInfo[]>([]);
  useEffect(() => {
    let disposed = false;
    authApi
      .providersList()
      .then((list) => { if (!disposed) setProviders(list); })
      .catch(() => {});
    return () => { disposed = true; };
  }, []);

  const clickable = providers.filter((p) => p.loginMode !== "planning");
  const planned = ["Claude", "Kiro"].map((name, i) => ({ name, key: `planned-${i}` }));

  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Auth 提供商">
      {clickable.map((provider) => {
        const active = selected === provider.id;
        const count = counts?.[provider.id] ?? 0;
        return (
          <button
            key={provider.id}
            type="button"
            onClick={() => onSelect(provider.id)}
            className={`inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-semibold shadow-sm transition-colors ${active ? "bg-success text-white" : "border border-border bg-muted text-muted-foreground hover:bg-muted/60 hover:text-foreground"}`}
            aria-pressed={active}
            title={provider.loginMode === "device_code" ? "设备码授权登录" : "浏览器 OAuth 授权登录"}
          >
            <Command size={13} className="hidden" />
            <span>{iconFor(provider.iconKey)}</span> {provider.displayName}
            {count > 0 && (
              <span
                className={`ml-0.5 inline-flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] font-bold leading-none ${active ? "bg-white/25 text-white" : "bg-primary/10 text-primary"}`}
                title={`${provider.displayName} 已配置 ${count} 个账号`}
              >
                {count}
              </span>
            )}
            {active && <span className="h-1.5 w-1.5 rounded-full bg-white/90" aria-hidden="true" />}
          </button>
        );
      })}
      {planned.length > 0 && <span className="mx-1 h-5 w-px bg-border" aria-hidden="true" />}
      {planned.map(({ name, key }) => (
        <span key={key} className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted px-3 py-1.5 text-xs text-muted-foreground">
          {name}
          <span className="rounded-full bg-card px-1.5 py-0.5 text-[10px]">规划中</span>
        </span>
      ))}
    </div>
  );
}
