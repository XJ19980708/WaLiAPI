import { useEffect, useRef, useState } from "react";
import { Check, Loader2, X } from "lucide-react";
import { authApi } from "../../lib/api";
import type { AuthAccount, AuthModelState } from "../../types";

export function ModelSyncModal({ account, onClose, onSynced }: { account: AuthAccount; onClose: () => void; onSynced: (models: AuthModelState[]) => void }) {
  const [models, setModels] = useState<AuthModelState[]>(account.models);
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(true);
  const syncedRef = useRef(onSynced);
  syncedRef.current = onSynced;
  useEffect(() => {
    let mounted = true;
    authApi.syncModels(account.id).then(next => { if (mounted) { setModels(next.models); syncedRef.current(next.models); } }).catch(() => { if (mounted) setError("模型同步失败，请稍后重试"); }).finally(() => mounted && setSyncing(false));
    return () => { mounted = false; };
  }, [account.id]);
  return <div className="fixed inset-0 z-50 flex items-center justify-center bg-foreground/35 p-4" role="dialog" aria-modal="true" aria-labelledby="sync-models-title">
    <div className="surface w-full max-w-lg rounded-[24px] p-6 shadow-2xl"><div className="flex items-start justify-between"><div><h2 id="sync-models-title" className="text-lg font-semibold">同步模型 · {account.label}</h2><p className="mt-1 text-sm text-muted-foreground">来自上游 /models 实时返回 · 登录/12h 自动同步 · 全量支持（只读）</p></div><button onClick={onClose} aria-label="关闭同步模型弹窗" className="rounded-lg p-1 text-muted-foreground hover:bg-muted"><X size={18} /></button></div>
      <div className="mt-5 rounded-2xl border border-border bg-muted/45 p-4">{syncing ? <p className="flex min-h-28 items-center gap-2 text-sm text-muted-foreground"><Loader2 size={16} className="animate-spin" />同步中…</p> : error ? <p role="alert" className="flex min-h-28 items-center text-sm text-destructive">{error}</p> : models.length ? <div><p className="mb-3 text-xs text-muted-foreground">共 {models.length} 个模型{models.some(model => model.unavailable) && `，其中 ${models.filter(model => model.unavailable).length} 个暂不可用`}</p><div className="grid max-h-64 grid-cols-2 gap-1.5 overflow-y-auto pr-1 sm:grid-cols-3">{models.map(model => <p key={model.id} title={model.unavailable ? `${model.id}（暂不可用）` : model.id} className={`flex items-center gap-1 truncate rounded-lg border border-border/60 bg-background/70 px-2 py-1 text-xs ${model.unavailable ? "text-muted-foreground" : ""}`}><Check size={12} className={`shrink-0 ${model.unavailable ? "text-muted-foreground" : "text-success"}`} /><span className="truncate">{model.id}</span></p>)}</div></div> : <p className="flex min-h-28 items-center text-sm text-muted-foreground">上游未返回可用模型。</p>}</div>
      <div className="mt-6 flex justify-end"><button onClick={onClose} className="action-secondary">关闭</button></div></div>
  </div>;
}
