export type Associations = { file?: string; folder?: string; folderExpanded?: string; rootFolder?: string; rootFolderExpanded?: string; rootFolderNames?: Record<string, string>; rootFolderNamesExpanded?: Record<string, string>; fileNames?: Record<string, string>; fileExtensions?: Record<string, string>; folderNames?: Record<string, string>; folderNamesExpanded?: Record<string, string>; languageIds?: Record<string, string> };
export type IconDefinition = { iconPath?: string; fontCharacter?: string; fontColor?: string; fontSize?: string; fontId?: string };
export interface IconTheme extends Associations {
  iconDefinitions?: Record<string, IconDefinition>;
  fonts?: Array<{ id: string; src?: Array<{ path?: string; format?: string }>; size?: string }>;
  light?: Associations;
  highContrast?: Associations;
}
export interface IconThemeList { selected: string | null; themes: Array<{ id: string; label: string }> }
