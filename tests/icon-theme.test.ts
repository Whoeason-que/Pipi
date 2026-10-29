import assert from "node:assert/strict";
import test from "node:test";
import { applyIconTheme, resolveIcon } from "../src/files/icon-theme.ts";

const icon = (name: string) => ({ iconPath: `data:image/svg+xml;base64,${name}` });
test("VS Code associations prefer names, parent and compound extensions, including variants and folders", () => {
  applyIconTheme({
    iconDefinitions: { default: icon("default"), named: icon("named"), parent: icon("parent"), ts: icon("ts"), dts: icon("dts"), dir: icon("dir"), open: icon("open"), light: icon("light"), contrast: icon("contrast") },
    file: "default", fileNames: { "package.json": "named", "src/package.json": "parent" },
    fileExtensions: { ts: "ts", "d.ts": "dts" }, folder: "dir", folderExpanded: "open",
    light: { fileExtensions: { ts: "light" } }, highContrast: { fileNames: { "package.json": "contrast" } },
  });
  assert.deepEqual(resolveIcon("src/package.json"), { image: "data:image/svg+xml;base64,parent" });
  assert.deepEqual(resolveIcon("docs/package.json"), { image: "data:image/svg+xml;base64,named" });
  assert.deepEqual(resolveIcon("index.d.ts"), { image: "data:image/svg+xml;base64,dts" });
  assert.deepEqual(resolveIcon("INDEX.TS", false, false, "light"), { image: "data:image/svg+xml;base64,light" });
  assert.deepEqual(resolveIcon("package.json", false, false, "highContrast"), { image: "data:image/svg+xml;base64,contrast" });
  assert.deepEqual(resolveIcon("src", true, false), { image: "data:image/svg+xml;base64,dir" });
  assert.deepEqual(resolveIcon("src", true, true), { image: "data:image/svg+xml;base64,open" });
  assert.deepEqual(resolveIcon("unknown"), { image: "data:image/svg+xml;base64,default" });
  applyIconTheme(null);
  assert.equal(resolveIcon("index.ts"), null);
});
