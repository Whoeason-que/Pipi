import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import ts from "typescript";

const root = path.resolve(import.meta.dirname, "..");
const read = (file: string) => readFileSync(path.join(root, file), "utf8");
const camel = (name: string) => name.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase());

function rustType(input: string): string {
  const value = input.trim().replace(/,$/, "");
  if (value.startsWith("Option<")) return rustType(value.slice(7, -1)) + "|null";
  if (value.startsWith("Vec<")) return rustType(value.slice(4, -1)) + "[]";
  if (value.startsWith("Result<")) return rustType(value.slice(7, -1).replace(/,\s*String$/, ""));
  const aliases: Record<string, string> = { String: "string", bool: "boolean", u64: "number", usize: "number", "()": "void", AgentDefinition: "AgentDefinition", Settings: "Settings", Model: "ModelConfig", PermissionsConfig: "PermissionsConfig", SessionInfo: "SessionInfoView", Message: "MessageView", "runtime::SessionSummary": "SessionSummaryView", "pipi_core::stats::SessionStats": "SessionStatsView", "pipi_protocol::BackgroundTaskSnapshot": "BackgroundTaskSnapshot", ModelCatalog: "ModelCatalog" };
  assert.ok(aliases[value], `unsupported command wire type: ${value}`);
  return aliases[value];
}

function desktopCommands() {
  const commands = new Map<string, { args: Array<{ name: string; optional: boolean; type: string }>; result: string }>();
  for (const file of ["src-tauri/src/commands.rs", "src-tauri/src/chat.rs"]) {
    for (const match of read(file).matchAll(/#\[tauri::command\]\s*pub (?:async )?fn (\w+)\((.*?)\)\s*(?:->\s*(.*?))?\s*\{/gs)) {
      const args = [...match[2].matchAll(/(\w+):\s*(.*?)(?=,\s*\w+:|,?\s*$)/gs)]
        .filter(arg => arg[1] !== "state" && arg[1] !== "app")
        .map(arg => ({ name: camel(arg[1]), optional: arg[2].trim().startsWith("Option<"), type: rustType(arg[2]) }));
      assert.equal(commands.has(match[1]), false, `duplicate command ${match[1]}`);
      commands.set(match[1], { args, result: rustType(match[3]) });
    }
  }
  return commands;
}

test("typed IPC map matches all desktop declarations, registrations and Web argument requirements", () => {
  const source = ts.createSourceFile("ipc.ts", read("src/ipc.ts"), ts.ScriptTarget.Latest, true);
  const map = source.statements.find(statement => ts.isInterfaceDeclaration(statement) && statement.name.text === "CommandMap");
  assert.ok(map && ts.isInterfaceDeclaration(map));
  const commands = desktopCommands();
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
  assert.deepEqual([...typed.keys()].sort(), [...commands.keys()].sort());
  for (const [name, args] of commands) assert.deepEqual(typed.get(name), args, name);
  const registrations = read("src-tauri/src/lib.rs").match(/generate_handler!\[([\s\S]*?)\]/)?.[1];
  assert.ok(registrations);
  const registered = [...registrations.matchAll(/(?:chat|commands)::(\w+)/g)].map(match => match[1]);
  assert.deepEqual(registered.sort(), [...commands.keys()].sort());
  const web = read("crates/pipi-server/src/main.rs").split("async fn invoke_command(")[1].split("fn required_string(")[0];
  const arms = [...web.matchAll(/"(\w+)"\s*=>/g)];
  assert.deepEqual(arms.map(arm => arm[1]).sort(), [...commands.keys()].sort());
  arms.forEach((arm, index) => {
    const body = web.slice(arm.index! + arm[0].length, arms[index + 1]?.index ?? web.indexOf('_ =>'));
    const args = [...body.matchAll(/(required|optional)_(?:string|value)(?:\s*::<[^>]+>)?\(args,\s*"(\w+)"\)/g)]
      .map(match => ({ name: match[2], optional: match[1] === "optional" }));
    assert.deepEqual(args.sort((a, b) => a.name.localeCompare(b.name)), commands.get(arm[1])!.args.map(({ name, optional }) => ({ name, optional })).sort((a, b) => a.name.localeCompare(b.name)), arm[1]);
  });
});

test("Rust wire fixtures fit frontend DTOs and typed invoke rejects misspelled commands and missing session identity", () => {
  const fixture = read("tests/fixtures/ipc-wire.json");
  const filename = path.join(root, "tests/fixtures/wire-check.ts");
  const code = `
import type { AgentDefinition, ModelConfig, Settings, SessionStatsView, SessionInfoView } from "../../src/types";
import type { MessageView } from "../../src/chat-runtime";
import type { BackgroundTaskSnapshot, EventMap, Invoke } from "../../src/ipc";
type Frame = { [K in keyof EventMap]: { type: K; payload: EventMap[K] } }[keyof EventMap];
const wire: { agent: AgentDefinition; model: ModelConfig; settings: Settings; messages: MessageView[]; stats: SessionStatsView; session: SessionInfoView; background: BackgroundTaskSnapshot; events: Frame[] } = ${fixture};
declare const invoke: Invoke;
invoke("session_info", {agentName:"fixture",sessionId:"session-1"});
invoke("list_agents");
// @ts-expect-error missing session identity
invoke("session_info", {agentName:"fixture"});
// @ts-expect-error misspelled command
invoke("session_inf", {agentName:"fixture",sessionId:"session-1"});
// @ts-expect-error wrong parameter type
invoke("stop_run", {agentName:"fixture",sessionId:42});
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
