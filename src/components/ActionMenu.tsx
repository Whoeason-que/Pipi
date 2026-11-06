import { useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { IconMore } from "../icons";

export interface MenuAction { label: string; run: () => void; danger?: boolean }

/**
 * 行尾「⋯」操作菜单。定位、碰撞翻转、焦点回收与键盘导航交给 Radix，
 * 外观仍是 `.action-menu` 自有样式；动作在菜单关闭后的下一个任务执行，
 * 避免菜单的滚动锁与接着打开的弹窗互相抢焦点。
 */
export default function ActionMenu({ label, actions }: { label: string; actions: MenuAction[] }) {
  const trigger = useRef<HTMLButtonElement>(null);
  const pending = useRef<(() => void) | null>(null);
  return (
    <DropdownMenu.Root
      onOpenChange={(open) => {
        if (open) return;
        const run = pending.current;
        pending.current = null;
        if (run) {
          setTimeout(() => {
            // 先把焦点还给触发按钮，再执行动作：接着打开的弹窗（如删除确认）
            // 才能在首次渲染时记住正确的返回目标。
            trigger.current?.focus();
            run();
          }, 0);
        }
      }}
    >
      <DropdownMenu.Trigger asChild>
        <button ref={trigger} type="button" className="icon-btn more-trigger" aria-label={label} title={label}>
          <IconMore />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="action-menu"
          aria-label={label}
          align="end"
          sideOffset={4}
          collisionPadding={8}
        >
          {actions.map((action) => (
            <DropdownMenu.Item
              key={action.label}
              className={action.danger ? "action-menu-item danger" : "action-menu-item"}
              onSelect={() => {
                pending.current = action.run;
              }}
            >
              {action.label}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
