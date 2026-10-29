import assert from "node:assert/strict";
import test from "node:test";
import { DEMO_WORKSPACE, demoWorkspaceChanges, demoWorkspaceDiff, demoWorkspaceFile, demoWorkspaceListing } from "../src/dev-workspace.ts";

test("demo workspace browses nested files and its git changes without accessing other workspaces", () => {
  const root = demoWorkspaceListing(DEMO_WORKSPACE, "");
  assert.ok(root.entries.some(entry => entry.name === "src" && entry.kind === "directory"));
  const nested = demoWorkspaceListing(DEMO_WORKSPACE, "src/components");
  assert.deepEqual(nested.entries.map(entry => entry.path), ["src/components/Status.tsx"]);
  assert.match(demoWorkspaceFile(DEMO_WORKSPACE, nested.entries[0].path).content!, /Status/);
  const changes = demoWorkspaceChanges(DEMO_WORKSPACE);
  for (const change of changes.files) {
    assert.match(demoWorkspaceDiff(DEMO_WORKSPACE, change.path).patch, /diff --git/);
    if (change.status !== " D") assert.equal(demoWorkspaceFile(DEMO_WORKSPACE, change.path).path, change.path);
  }
  assert.throws(() => demoWorkspaceFile("/other", "README.md"));
  assert.throws(() => demoWorkspaceFile(DEMO_WORKSPACE, "../README.md"));
});
