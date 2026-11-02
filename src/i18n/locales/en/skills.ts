export const skills = {
  // 技能库主区
  install: "Install skill",
  installAction: "Install",
  verifyAll: "Verify all",
  verifyAllHint: "Recompute store content hashes and check for missing or drifted content",
  verifyContent: "Verify content",
  detailsTitle: "Skill details",
  toggleDetails: "Toggle skill details panel",
  loading: "Reading global skill store…",
  loadError: "Failed to read the skill store",
  locationHint: "Location: ~/.av/skills (override with AV_HOME)",
  storeTitle: "Global skill store",
  storeDescription:
    "Installing places a capability pack in the local store; it enters a session’s prompt only after you enable it in the Agent workspace.",
  installed: "Installed skills",
  contentVersions: "Content versions",
  contentIssues: "Content issues",
  storage: "Storage",
  localFiles: "Local files",
  storeEmptyTitle: "No skills in the store yet",
  storeEmptyHint:
    "Install your first skill from a GitHub shorthand (owner/repo), a git URL, or a local directory.",
  viewingSkill: "Viewing {name}; details are on the right",
  selectCardHint: "Select a card to see versions, sources, and the Agents that enable it",
  rulesTitle: "Store and injection rules",
  rulesInstallTitle: "Install ≠ enable",
  rulesInstallBefore: ": installing only puts content into the store; declare",
  rulesInstallAfter: " on the Agent’s Skills page to enable it",
  rulesReadTitle: "Session start is read-only",
  rulesReadBefore: ": the runtime resolves local content only from the adjacent ",
  rulesReadAfter:
    "and never goes online or installs silently; a missing lock or missing content fails closed",
  rulesCli: "Equivalent CLI commands:",
  noDescription: "(No readable description in SKILL.md)",
  versionCount: "{count} versions",
  currentVersion: "Current version",
  currentVersionPresent: "Current version content is present",
  currentVersionMissing: "Current version content is missing",
  content: "Content",
  present: "Present",
  missing: "Missing",
  contentMissing: "Content missing",
  removeTitle: "Remove skill from store",
  removeMessage:
    "Deletes all versions of {name} from the store (irreversible). It is refused while an Agent still declares the skill; disable it on the Agent’s Skills page first.",
  remove: "Remove",
  noticeVerifiedAll: "Verified {count} versions: content matches the index",
  noticeVerifyIssues:
    "Some versions have missing or drifted content (see the list markers); drift is only cleared by reinstalling the source",
  noticeRemoved: "Removed {name} from the store ({count} versions)",

  // 技能库侧栏
  searchInstalled: "Search installed skills",
  searchPlaceholder: "Search installed skills…",
  reloadStore: "Reload store",
  loadingStore: "Reading global store…",
  storeEmptySidebar: "Store is empty: install your first skill from the main area",
  noMatches: "No matching skills",

  // 技能右栏
  selectSkillHint: "Select a skill in the main area",
  tabOverview: "Overview",
  tabUsage: "Enabled",
  noSelection: "No skill selected",
  noSelectionDescription:
    "Select a skill card in the main area to see versions, sources, verification results, and the Agents that enable it.",
  sourcePath: "Path in source",
  installedAt: "Installed at",
  versionsHeading: "Versions · {count}",
  current: "Current",
  actualHash: "Actual hash: {hash}",
  verifyNote:
    "Verification recomputes store content hashes; drift is only cleared by reinstalling the source.",
  loadingUsage: "Reading enablement…",
  noAgents: "No Agents yet",
  noAgentsDescription: "Create an Agent to enable skills here.",
  agentsHeading: "Agents · {count} enabled",
  notEnabled: "Not enabled",
  enable: "Enable",
  disable: "Disable",
  enableNote:
    "Enabling writes the highest declaration layer in effect for this Agent and the matching agent.lock; it applies when the next session starts.",

  // Agent 技能页
  syncReportResolved: "Resolved locks: {names}",
  syncReportInstalled: "Restored content: {names}",
  syncReportRepaired: "Reinstalled drift: {names}",
  syncReportReady: "Ready: {names}",
  loadDeclarationsFailed: "Failed to read skill declarations: {error}",
  loadingDeclarations: "Reading skill declarations…",
  declaredByProjectLocal: "Project-local agent.local.toml",
  declaredByProject: "Project agent.toml",
  declaredByAgent: "Agent-layer agent.toml",
  enabledCount: "{count} skills enabled",
  noneEnabled: "No skills enabled",
  overviewDescription:
    "Skill content comes from the global store; this page only manages the current Agent’s declarations and resolution lock.",
  declaration: "Declaration",
  projectLayer: "Project layer",
  projectLayerHint:
    "Project-layer declarations override the Agent layer; edits write to files in the workspace",
  explicitlyDisabled: "{source} (use = [], explicitly disabled)",
  noUseDeclared: "No use declared",
  contract: "Contract",
  lock: "Resolution lock",
  enabledHeading: "Enabled ({count})",
  noEnabledSkills: "No skills are declared as enabled",
  availableHeading: "Available ({count} in store)",
  noAvailable:
    "No other skills in the store. Install from the Skills section on the left (installing ≠ enabling).",
  enableHint: "Writes use and generates the resolution lock",
  enableMissingHint: "Store content is missing: install or sync first",
  repair: "Repair",
  syncDeclared: "Sync declared skills",
  syncing: "Syncing…",
  repairDrift: "Repair drift",
  syncHint:
    "Restore missing content from agent.lock (git sources go online); content that does not match the lock is only reinstalled when Repair drift is on.",
  filtersHeading: "Filters (only / exclude / sources)",
  onlyHint: "Only inject matching skill names (glob)",
  excludeHint: "Matching skill names are not injected (takes precedence over only)",
  sourcesHint: "Replace the conventional skill directories per layer",
  toggleHint:
    "Enabling or disabling writes the contract and agent.lock immediately (same implementation as av skill add/remove), with no save step. Running sessions are unaffected; changes apply when the next session starts.",

  // 安装弹层
  modeAuto: "Auto (source contains exactly one skill)",
  modeAll: "All skills",
  modeOnly: "Specific skill names",
  sourceRequired: "Enter a skill source (owner/repo[@ref][#subdir], git URL, or local directory)",
  namesRequired: "Enter the skill names to install, or switch to the Auto / All strategy",
  sourceLabel: "Source",
  sourcePlaceholder: "owner/repo@ref#subdir or /local/skill/dir",
  sourceHint:
    "Git sources are shallow-fetched to the exact commit with system git (goes online and may take a while); local directories are copied as a content snapshot.",
  selectionMode: "Selection strategy",
  skillNames: "Skill names",
  namesHint:
    "Separate with commas or spaces; every name must exist in the source or the whole install is rejected.",
  dryRun: "Preview only (dry-run, writes nothing)",
  upToDate: "Up to date",
  willInstall: "Will install",
  installedDone: "Installed",
  skipped: "Skipped",
  installNotEnableHint:
    "Install ≠ enable: placing content in the store only makes it available; sessions get it after you enable it on the Agent’s Skills page.",
  done: "Done",
  working: "Working… (git sources go online)",
  preview: "Preview",

  // 状态标签（labels.ts 返回的 i18n key）
  status: {
    ready: "Ready",
    missing: "Content missing",
    broken: "Content unparsable",
    unresolved: "Unresolved",
    filtered: "Filtered",
  },
  statusHint: {
    ready: "Lock record and store content are both present; sessions will inject this skill",
    missing:
      "The lock resolves it but the store has no matching content: sync can reproduce it, and sessions fail until fixed",
    broken: "Content exists but SKILL.md is missing or unparsable: sessions will fail",
    unresolved:
      "The contract declares use, but the adjacent agent.lock has no resolution record: run sync",
    filtered: "Filtered out by only / exclude; it will not enter the system prompt",
  },
  verify: {
    ok: "Match",
    missing: "Missing",
    mismatch: "Drift",
  },
  source: {
    local: "Local {path}",
  },
};
