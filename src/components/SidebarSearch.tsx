import { IconClose, IconSearch } from "../icons";

/**
 * 侧栏顶部的搜索/筛选框。
 *
 * 五个侧栏（搜索、论坛、工作区、文件、技能）共用同一套 `.sb-filter` 结构；
 * 样式早已统一，这里只把重复的 JSX 收成一处，差异通过 props 传入。
 */
export function SidebarSearch({
  value,
  onChange,
  placeholder,
  label,
  clearLabel = "清空搜索",
  autoFocus = false,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  label: string;
  clearLabel?: string;
  autoFocus?: boolean;
}) {
  return (
    <div className="sb-filter">
      <div className="search">
        <IconSearch />
        <input
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder={placeholder}
          aria-label={label}
          autoComplete="off"
          spellCheck={false}
          autoFocus={autoFocus}
        />
        {value && (
          <button
            type="button"
            className="search-clear"
            title={clearLabel}
            aria-label={clearLabel}
            onClick={() => onChange("")}
          >
            <IconClose />
          </button>
        )}
      </div>
    </div>
  );
}
