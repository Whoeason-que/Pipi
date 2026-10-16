import {
  IconArrowRepeat,
  IconGear,
  IconGrid,
  IconKey,
  IconLayers,
} from "../icons";

export type SettingsSection =
  | "appearance"
  | "context"
  | "retry"
  | "providers"
  | "account";

export function getSettingsSectionInfo(
  section: SettingsSection,
  t: (key: string) => string,
) {
  return {
    label: t(`settings.${section}`),
    description: t(`settings.${section}Desc`),
  };
}

const SECTION_ICONS = {
  appearance: IconGear,
  context: IconLayers,
  retry: IconArrowRepeat,
  providers: IconGrid,
  account: IconKey,
};

export function SettingsSectionIcon({ section }: { section: SettingsSection }) {
  const Icon = SECTION_ICONS[section];
  return <Icon />;
}
