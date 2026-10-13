//! 模型目录的 React 取数封装：统一「先读进程内缓存，没有就拉取」的加载状态。
//! 取数语义在 catalog-client.ts（IO 层），本模块只管加载状态与重试出口；
//! 错误怎么展示由调用方决定（ModelPicker 写进提示行，其余静默退化为手填）。
import { useCallback, useEffect, useState } from "react";
import type { ModelCatalog } from "./catalog";
import { loadCatalog, peekCatalog } from "./catalog-client";

export interface CatalogState {
  /** 已加载的目录（尚未加载完成时为 null） */
  catalog: ModelCatalog | null;
  loading: boolean;
  error: string | null;
  /** 失败后的重试出口：强制联网重新拉取 */
  reload: () => void;
}

export function useCatalog(): CatalogState {
  const [catalog, setCatalog] = useState<ModelCatalog | null>(peekCatalog());
  const [loading, setLoading] = useState(peekCatalog() === null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    // 首次挂载已有缓存：不再拉取（重试则忽略缓存，走 force refresh）
    if (attempt === 0 && peekCatalog()) return;
    let alive = true;
    setLoading(true);
    loadCatalog(attempt > 0)
      .then((next) => {
        if (!alive) return;
        setCatalog(next);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (alive) setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [attempt]);

  const reload = useCallback(() => setAttempt((value) => value + 1), []);
  return { catalog, loading, error, reload };
}
