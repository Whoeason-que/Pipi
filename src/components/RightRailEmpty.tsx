import type { ReactNode } from "react";

/** 右栏统一空态：保持 `.right-rail-empty > svg/strong/p` 的既有 DOM 结构。 */
export function RightRailEmpty({
  icon,
  title,
  description,
}: {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
}) {
  return (
    <div className="right-rail-empty">
      {icon}
      <strong>{title}</strong>
      {description ? <p>{description}</p> : null}
    </div>
  );
}
