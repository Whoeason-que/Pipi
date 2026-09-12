//! react-select 的统一封装：样式走我们自己的 CSS（`unstyled` + `.pipi-select__*`），
//! 主题变量与其它手写组件一致，不让第三方样式体系渗进来。
//!
//! 只暴露「字符串值 + 选项数组」这一层，调用方不必接触 react-select 的类型。
import Select from "react-select";
import CreatableSelect from "react-select/creatable";

export interface Choice {
  value: string;
  label: string;
}

export interface ChoiceGroup {
  label: string;
  options: Choice[];
}

interface ChoiceProps {
  value: string;
  /** 平铺选项（与 groups 二选一） */
  choices?: Choice[];
  /** 分组选项（与 choices 二选一；用于预设/供应商这类需要分组的场景）。 */
  groups?: ChoiceGroup[];
  onChange: (value: string) => void;
  id?: string;
  placeholder?: string;
  disabled?: boolean;
  isClearable?: boolean;
  /** 挂在 body 上，避免被弹窗容器裁掉。 */
  menuInPortal?: boolean;
  autoFocus?: boolean;
  ariaLabel?: string;
}

const PORTAL_STYLES = { menuPortal: (base: Record<string, unknown>) => ({ ...base, zIndex: 80 }) };

function currentOption(value: string, options: Choice[]): Choice | null {
  if (!value) return null;
  const hit = options.find((option) => option.value === value);
  // 值不在选项里时也要显示出来（例如目录里下线了的历史模型），否则下拉会显示成别的值
  return hit ?? { value, label: value };
}

function flatten(choices: Choice[], groups?: ChoiceGroup[]): Choice[] {
  return groups ? groups.flatMap((group) => group.options) : choices;
}

/** 单选下拉：目录/供应商这类「给定的可选项」。 */
export function ChoiceSelect({
  value,
  choices = [],
  groups,
  onChange,
  id,
  placeholder,
  disabled,
  isClearable,
  menuInPortal,
  autoFocus,
  ariaLabel,
}: ChoiceProps) {
  const all = flatten(choices, groups);
  return (
    <Select<Choice, false>
    inputId={id}
    classNamePrefix="pipi-select"
    unstyled
    isSearchable
    isClearable={isClearable}
    isDisabled={disabled}
    autoFocus={autoFocus}
    placeholder={placeholder}
    aria-label={ariaLabel}
    value={currentOption(value, all)}
    options={groups ?? choices}
    onChange={(option) => onChange(option ? option.value : "")}
    menuPortalTarget={menuInPortal && typeof document !== "undefined" ? document.body : undefined}
    styles={menuInPortal ? PORTAL_STYLES : undefined}
    noOptionsMessage={() => "没有匹配项"}
  />
  );
}

/** 可创建下拉：目录里没有的模型 ID 必须能手填（例如自建端点、目录未收录的模型）。 */
export function ChoiceCreatable({
  value,
  choices = [],
  onChange,
  id,
  placeholder,
  disabled,
  isClearable,
  menuInPortal,
  ariaLabel,
  createLabel,
}: ChoiceProps & { createLabel?: (input: string) => string }) {
  return (
    <CreatableSelect<Choice, false>
      inputId={id}
      classNamePrefix="pipi-select"
      unstyled
      isSearchable
      isClearable={isClearable}
      isDisabled={disabled}
      placeholder={placeholder}
      aria-label={ariaLabel}
      value={currentOption(value, choices)}
      options={choices}
      onChange={(option) => onChange(option ? option.value : "")}
      onCreateOption={(input) => onChange(input.trim())}
      isValidNewOption={(input) => input.trim().length > 0}
      formatCreateLabel={(input) => (createLabel ? createLabel(input) : `使用自定义值 ${input}`)}
      menuPortalTarget={menuInPortal && typeof document !== "undefined" ? document.body : undefined}
      styles={menuInPortal ? PORTAL_STYLES : undefined}
      noOptionsMessage={() => "目录里没有：直接输入模型 ID 回车即可"}
    />
  );
}
