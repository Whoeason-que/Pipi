import type { Ref } from "react";

export type ScreenView = "chat" | "detail";

/**
 * 主区顶部的「会话 / 详情」切换。
 * 独立成文件，避免 App 与 Chat 互相 import 形成循环依赖。
 * 语义上是一组切换按钮（aria-pressed），不使用 tablist：
 * 同一时刻只渲染其中一个视图，创建不存在对应 panel 的 tab 是无障碍语义错误。
 */
export function ScreenTabs({
  active,
  onSelect,
  innerRef,
}: {
  active: ScreenView;
  onSelect: (view: ScreenView) => void;
  innerRef?: Ref<HTMLDivElement>;
}) {
  return (
    <div className="tabs" role="group" aria-label="视图切换" ref={innerRef}>
      <button
        type="button"
        aria-pressed={active === "chat"}
        className={`tab${active === "chat" ? " active" : ""}`}
        onClick={() => onSelect("chat")}
      >
        会话
      </button>
      <button
        type="button"
        aria-pressed={active === "detail"}
        className={`tab${active === "detail" ? " active" : ""}`}
        onClick={() => onSelect("detail")}
      >
        详情
      </button>
    </div>
  );
}
