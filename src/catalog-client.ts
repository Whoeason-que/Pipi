//! 模型目录的取数层：调用 pipi-core 的 `model_catalog` 命令（models.dev + 本地缓存），
//! 并在进程内缓存结果。纯变换在 catalog.ts，这里只做 IO —— 测试不导入本模块。
import type { ModelCatalog } from "./catalog";
import { invoke } from "./platform";

let memory: ModelCatalog | null = null;
let inflight: Promise<ModelCatalog> | null = null;

/** 取目录（进程内缓存；`refresh` 强制联网刷新）。失败时抛错，由 UI 退化为手填。 */
export async function loadCatalog(refresh = false): Promise<ModelCatalog> {
  if (!refresh && memory) return memory;
  if (!refresh && inflight) return inflight;
  const request = invoke<ModelCatalog>("model_catalog", { refresh })
    .then((catalog) => {
      memory = catalog;
      return catalog;
    })
    .finally(() => {
      if (inflight === request) inflight = null;
    });
  inflight = request;
  return request;
}

/** 已加载的目录（未加载返回 null）——给同步渲染的组件用。 */
export function peekCatalog(): ModelCatalog | null {
  return memory;
}

/** 测试/切换账号等场景：清掉进程内缓存。 */
export function resetCatalog(): void {
  memory = null;
  inflight = null;
}
