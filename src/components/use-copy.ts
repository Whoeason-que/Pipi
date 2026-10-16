import { useEffect, useRef, useState } from "react";

/** 消息与代码复制使用同一套真实结果反馈；失败时提供手动复制指引。 */
export function useCopyFeedback() {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; clearTimeout(timer.current); };
  }, []);
  const copy = async (text: string) => {
    let result: "copied" | "failed" = "failed";
    try {
      if (!navigator.clipboard?.writeText) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(text);
      result = "copied";
    } catch { /* 只有实际成功才显示已复制 */ }
    if (!mounted.current) return;
    setState(result); clearTimeout(timer.current);
    timer.current = setTimeout(() => setState("idle"), result === "copied" ? 1800 : 5000);
  };
  return { state, copy, label: state === "copied" ? "已复制" : state === "failed" ? "复制失败，请选中文本手动复制" : "复制" };
}
