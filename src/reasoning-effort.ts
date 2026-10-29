import type { ReasoningEffort } from "./types.ts";

export function reasoningChoices(en: boolean): { value: ReasoningEffort | "default"; label: string }[] {
  return [
    { value: "default", label: en ? "Model default" : "模型默认" },
    { value: "none", label: en ? "Off" : "关闭" },
    { value: "minimal", label: en ? "Minimal" : "极低" },
    { value: "low", label: en ? "Low" : "低" },
    { value: "medium", label: en ? "Medium" : "中" },
    { value: "high", label: en ? "High" : "高" },
    { value: "xhigh", label: en ? "Extra high" : "极高" },
    { value: "max", label: en ? "Maximum" : "最高" },
  ];
}
