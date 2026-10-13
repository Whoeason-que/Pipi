import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import ts from "typescript";

const root = path.resolve(import.meta.dirname, "..");
const read = (file: string) => readFileSync(path.join(root, file), "utf8");
const camel = (name: string) => name.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase());
const pascal = (name: string) => name.replace(/(^|_)([a-z])/g, (_, __, letter: string) => letter.toUpperCase());

function rustType(input: string): string {
  const value = input.trim().replace(/\s+/g, " ").replace(/,$/, "");
  if (value.startsWith("Option<")) return rustType(value.slice(7, -1)) + "|null";
  if (value.startsWith("Vec<")) return rustType(value.slice(4, -1)) + "[]";
  if (value.startsWith("Result<")) return rustType(value.slice(7, -1).replace(/,\s*String$/, ""));
  const aliases: Record<string, string> = { String: "string", bool: "boolean", u64: "number", usize: "number", "()": "void", AgentDefinition: "AgentDefinition", Settings: "Settings", Model: "ModelConfig", PermissionsConfig: "PermissionsConfig", SessionInfo: "SessionInfoView", SessionSnapshot: "SessionSnapshotView", Message: "MessageView", SessionSummary: "SessionSummaryView", SessionImportReport: "SessionImportReportView", SessionStats: "SessionStatsView", BackgroundTaskSnapshot: "BackgroundTaskSnapshot", ModelCatalog: "ModelCatalog", SkillLibrary: "SkillLibrary", AgentSkills: "AgentSkills", AgentEditableFiles: "AgentEditableFiles", SkillVerify: "SkillVerify", SkillInstallReport: "SkillInstallReport", SkillSyncReport: "SkillSyncReport", SkillUsage: "SkillUsage" };
  assert.ok(aliases[value], `unsupported command wire type: ${value}`);
  return aliases[value];
}

/** 共享命令表：crates/pipi-core/src/commands.rs 是宿主命令的唯一事实来源。 */
function sharedCommands() {
  const source = read("crates/pipi-core/src/commands.rs");
  const commandNames = [...source
    .match(/pub const COMMANDS: &\[&str\] = &\[([\s\S]*?)\];/)![1]
    .matchAll(/"(\w+)"/g)].map(match => match[1]);
  const returns = new Map<string, string>();
  for (const match of source.matchAll(/pub (?:async )?fn (\w+)\(([^)]*)\)\s*->\s*([^{]+?)\s*\{/g)) {
    returns.set(match[1], match[3]);
  }
  const structs = new Map<string, Array<{ name: string; optional: boolean; type: string }>>();
  for (const match of source.matchAll(/pub struct (\w+) \{([\s\S]*?)\}/g)) {
    structs.set(match[1], [...match[2].matchAll(/pub (\w+):\s*([^,]+),/g)].map(field => ({
      name: camel(field[1]),
      optional: field[2].trim().startsWith("Option<"),
      type: rustType(field[2]),
    })));
  }
  const commands = new Map<string, { args: Array<{ name: string; optional: boolean; type: string }>; result: string }>();
  for (const name of commandNames) {
    assert.equal(commands.has(name), false, `COMMANDS 重复命令 ${name}`);
    const result = returns.get(name);
    assert.ok(result, `commands.rs 缺少 ${name} 的命令函数`);
    const args = structs.get(pascal(name) + "Args") ?? [];
    assert.equal(args.length === 0, structs.get(pascal(name) + "Args") === undefined, `${name} 的空参数结构体应省略`);
    commands.set(name, { args, result: rustType(result) });
  }
  return { source, commandNames, commands };
}

test("shared command table matches the typed IPC map, the Tauri shells and the Web dispatch table", () => {
  const source = ts.createSourceFile("ipc.ts", read("src/ipc.ts"), ts.ScriptTarget.Latest, true);
  const map = source.statements.find(statement => ts.isInterfaceDeclaration(statement) && statement.name.text === "CommandMap");
  assert.ok(map && ts.isInterfaceDeclaration(map));
  const typed = new Map(map.members.map(member => {
    assert.ok(ts.isPropertySignature(member) && member.type && ts.isTypeLiteralNode(member.type));
    const args = member.type.members.find(field => field.name?.getText(source) === "args");
    assert.ok(args && ts.isPropertySignature(args) && args.type && ts.isTypeLiteralNode(args.type));
    const result = member.type.members.find(field => field.name?.getText(source) === "result");
    assert.ok(result && ts.isPropertySignature(result) && result.type);
    return [member.name.getText(source), { result: result.type.getText(source).replace(/\s/g, ""), args: args.type.members.map(arg => {
      assert.ok(ts.isPropertySignature(arg));
      return { name: arg.name.getText(source), optional: Boolean(arg.questionToken), type: arg.type!.getText(source).replace(/\s/g, "") };
    }) }] as const;
  }));

  const { source: shared, commandNames, commands } = sharedCommands();
  assert.deepEqual(commandNames, [...typed.keys()], "COMMANDS 顺序必须与 ipc.ts CommandMap 一致");
  for (const [name, args] of commands) {
    const expected = typed.get(name);
    assert.ok(expected, `commands.rs 多出命令 ${name}`);
    assert.equal(args.result, expected.result, name);
    const declared = [...args.args].sort((a, b) => a.name.localeCompare(b.name));
    const ipc = [...expected.args].sort((a, b) => a.name.localeCompare(b.name));
    assert.deepEqual(declared, ipc, name);
  }
  for (const name of typed.keys()) assert.ok(commands.has(name), `commands.rs 缺少命令 ${name}`);

  // 桌面壳：每个命令一个 #[tauri::command] 薄壳，且转发到同名共享函数。
  const tauriFiles = ["crates/pipi/src/commands.rs", "crates/pipi/src/chat.rs"];
  const declared = tauriFiles.flatMap(file => [...read(file).matchAll(/#\[tauri::command\]\s*pub (?:async )?fn (\w+)\(/g)].map(match => match[1]));
  assert.deepEqual(declared.sort(), [...commandNames].sort(), "Tauri 壳必须声明全部宿主命令");
  const forwarded = tauriFiles.flatMap(file => [...read(file).matchAll(/pipi_core::commands::(\w+)/g)].map(match => match[1]));
  assert.deepEqual([...new Set(forwarded)].sort(), [...commandNames].sort(), "Tauri 壳必须转发到共享函数");

  // Web 端：dispatch 表覆盖全部命令，每个分支调用同名共享函数。
  const dispatch = shared.match(/pub async fn dispatch\([\s\S]*?\n\}/)?.[0];
  assert.ok(dispatch, "commands.rs 必须包含 pub async fn dispatch");
  const arms = [...dispatch.matchAll(/"(\w+)"\s*=>/g)];
  assert.deepEqual(arms.map(arm => arm[1]), commandNames, "dispatch 表必须覆盖全部命令");
  arms.forEach((arm, index) => {
    const body = dispatch.slice(arm.index! + arm[0].length, arms[index + 1]?.index ?? dispatch.length);
    assert.ok(body.includes(`${arm[1]}(`), `dispatch 的 ${arm[1]} 分支未调用同名函数`);
  });

  const registrations = read("crates/pipi/src/lib.rs").match(/generate_handler!\[([\s\S]*?)\]/)?.[1];
  assert.ok(registrations);
  const registered = [...registrations.matchAll(/(?:chat|commands)::(\w+)/g)].map(match => match[1]);
  assert.deepEqual(registered.sort(), [...commandNames].sort(), "Tauri 注册表必须覆盖全部命令");

  const web = read("crates/pipi-core/src/server/mod.rs");
  assert.match(web, /crate::commands::dispatch\(/, "Web 端必须调用共享 dispatch");
  assert.doesNotMatch(web, /fn (?:required|optional)_(?:string|value)\(/, "Web 端不得再手写参数解析");
});

/** 从 `fn name(` 的左括号扫描到配对右括号，返回形参表原文（括号可出现在类型里，不能靠正则截断）。 */
function paramList(source: string, openParen: number): string {
  let depth = 0;
  for (let index = openParen; index < source.length; index++) {
    if (source[index] === "(") depth++;
    else if (source[index] === ")") {
      depth--;
      if (depth === 0) return source.slice(openParen + 1, index);
    }
  }
  throw new Error("unbalanced parameter list");
}

test("front-end guards and the Tauri shells keep the shapes the command table implies", () => {
  const { commands } = sharedCommands();

  // 前端运行时兜底表（platform.ts）必须与「参数里带 agentName 的命令」一一对应，
  // 且 sessionId 的必填/可选与 args 结构体的 String / Option<String> 一致。
  const tableSource = read("src/platform.ts")
    .match(/const AGENT_COMMAND_ARGS: Record<string, \{[^}]*\}> = \{([\s\S]*?)\n\};/)?.[1];
  assert.ok(tableSource, "platform.ts 必须有 AGENT_COMMAND_ARGS 兜底表");
  const guard = new Map<string, "required" | "optional" | undefined>();
  for (const match of tableSource.matchAll(/^\s*(\w+):\s*\{([^}]*)\},?\s*$/gm)) {
    assert.equal(guard.has(match[1]), false, `兜底表重复命令 ${match[1]}`);
    guard.set(match[1], match[2].match(/sessionId:\s*"(\w+)"/)?.[1] as "required" | "optional" | undefined);
  }
  const expected = new Map<string, "required" | "optional" | undefined>();
  for (const [name, { args }] of commands) {
    if (!args.some(arg => arg.name === "agentName")) continue;
    const sessionId = args.find(arg => arg.name === "sessionId");
    expected.set(name, sessionId ? (sessionId.optional ? "optional" : "required") : undefined);
  }
  assert.ok(expected.size > 0);
  assert.deepEqual([...guard.keys()].sort(), [...expected.keys()].sort(), "兜底表必须覆盖且只覆盖带 agentName 的命令");
  for (const [name, sessionId] of expected) {
    assert.equal(guard.get(name), sessionId, `${name} 的 sessionId 要求与命令表不一致`);
  }

  // 桌面壳：带参命令必须用 InvokeArgs<XxxArgs> 扁平反序列化（裸 XxxArgs 会去找
  // payload.args，与前端实际发送的形状不符）；带参命令数与命令表一致，防止循环落空。
  const tauriFiles = ["crates/pipi/src/commands.rs", "crates/pipi/src/chat.rs"];
  const withArgs: string[] = [];
  for (const file of tauriFiles) {
    const source = read(file);
    for (const match of source.matchAll(/#\[tauri::command\]\s*pub (?:async )?fn (\w+)\(/g)) {
      const params = paramList(source, match.index! + match[0].length - 1);
      if (!params.includes("args:")) continue;
      withArgs.push(match[1]);
      assert.match(params, /args:\s*InvokeArgs</, `${file} 的 ${match[1]} 必须用 InvokeArgs<XxxArgs>`);
    }
  }
  const expectedWithArgs = [...commands].filter(([, command]) => command.args.length > 0).map(([name]) => name);
  assert.deepEqual(withArgs.sort(), expectedWithArgs.sort(), "带参命令必须与命令表一致");
});

test("Rust wire fixtures fit frontend DTOs and typed invoke rejects misspelled commands and missing session identity", () => {
  const fixture = read("tests/fixtures/ipc-wire.json");
  const filename = path.join(root, "tests/fixtures/wire-check.ts");
  const code = `
import type { AgentDefinition, AgentEditableFiles, AgentSkills, ModelConfig, Settings, SessionStatsView, SessionInfoView, SkillLibrary } from "../../src/types";
import type { MessageView } from "../../src/chat-runtime";
import type { BackgroundTaskSnapshot, EventMap, Invoke } from "../../src/ipc";
type Frame = { [K in keyof EventMap]: { type: K; payload: EventMap[K] } }[keyof EventMap];
const wire: { agent: AgentDefinition; agentFiles: AgentEditableFiles; model: ModelConfig; settings: Settings; messages: MessageView[]; stats: SessionStatsView; session: SessionInfoView; background: BackgroundTaskSnapshot; skillLibrary: SkillLibrary; agentSkills: AgentSkills; events: Frame[] } = ${fixture};
declare const invoke: Invoke;
invoke("session_info", {agentName:"fixture",sessionId:"session-1"});
invoke("list_agents");
invoke("set_skill_enabled", {agentName:"fixture",name:"pdf",enabled:true});
invoke("install_skill", {source:"example/skills",all:false,dryRun:true});
invoke("agent_skills", {agentName:"fixture"});
invoke("list_agent_files", {agentName:"fixture"});
// @ts-expect-error missing session identity
invoke("session_info", {agentName:"fixture"});
// @ts-expect-error misspelled command
invoke("session_inf", {agentName:"fixture",sessionId:"session-1"});
// @ts-expect-error wrong parameter type
invoke("stop_run", {agentName:"fixture",sessionId:42});
// @ts-expect-error skill toggle needs a boolean
invoke("set_skill_enabled", {agentName:"fixture",name:"pdf",enabled:"yes"});
// @ts-expect-error result is inferred from the command
const result: Promise<boolean> = invoke("session_info", {agentName:"fixture",sessionId:"session-1"});
void wire;
`;
  const options: ts.CompilerOptions = { noEmit: true, strict: true, skipLibCheck: true, target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler };
  const host = ts.createCompilerHost(options);
  const original = host.getSourceFile.bind(host);
  host.getSourceFile = (file, languageVersion, onError, shouldCreate) => file === filename
    ? ts.createSourceFile(file, code, languageVersion, true) : original(file, languageVersion, onError, shouldCreate);
  const program = ts.createProgram([filename], options, host);
  const diagnostics = ts.getPreEmitDiagnostics(program);
  assert.deepEqual(diagnostics.map(diagnostic => ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n")), []);
});
