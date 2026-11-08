import assert from "node:assert/strict";
import test from "node:test";
import { buildSessionGraph, buildSessionTree, visibleSessions, type SessionTreeNode } from "../src/chat/session-tree.ts";
import type { SessionSummaryView } from "../src/types.ts";

function session(
  id: string,
  lastActive: number,
  forkParentId?: string,
): SessionSummaryView {
  return {
    id,
    title: id,
    messageCount: 0,
    startedAt: 0,
    lastActive,
    forkParentId,
  };
}

function ids(nodes: SessionTreeNode[]): string[] {
  return nodes.flatMap((node) => [node.session.id, ...ids(node.children)]);
}

test("archive visibility hides the entire descendant chain and restores the full graph", () => {
  const sessions = [session("grandchild", 5, "child"), session("child", 4, "root"), session("root", 3), session("other", 2)];
  const archived = new Set(["root"]);
  assert.deepEqual(visibleSessions(sessions, archived, false).map((item) => item.id), ["other"]);
  const graph = buildSessionGraph(buildSessionTree(visibleSessions(sessions, archived, true)));
  assert.deepEqual(graph.map((row) => row.node.session.id), ["grandchild", "child", "root", "other"]);
  assert.equal(graph[0].output[0].targetId, "child");
  assert.equal(graph[1].output[0].targetId, "root");
  assert.deepEqual(visibleSessions(sessions, new Set(), false).map((item) => item.id), sessions.map((item) => item.id));
});

test("archive visibility hides only the affected branch and handles cycles", () => {
  const sessions = [session("a", 5, "b"), session("b", 4, "a"), session("child", 3, "root"), session("sibling", 2, "root"), session("root", 1)];
  assert.deepEqual(visibleSessions(sessions, new Set(["a", "child"]), false).map((item) => item.id), ["sibling", "root"]);
});

test("nests forks under their source and keeps independent branches as roots", () => {
  const tree = buildSessionTree([
    session("fork", 30, "source"),
    session("source", 20),
    session("other", 10),
  ]);
  assert.deepEqual(tree.map((node) => node.session.id), ["source", "other"]);
  assert.equal(tree[0].children[0].session.id, "fork");
  assert.equal(tree[0].missingParentId, undefined);
});

test("a fork whose source is gone becomes a root with the missing source id", () => {
  const tree = buildSessionTree([session("child", 10, "archived")]);
  assert.equal(tree.length, 1);
  assert.equal(tree[0].session.id, "child");
  assert.equal(tree[0].missingParentId, "archived");
});

test("chains nest and sibling order follows the input (lastActive desc)", () => {
  const tree = buildSessionTree([
    session("grandchild", 4, "child"),
    session("child", 3, "root"),
    session("root", 2),
    session("sibling", 1, "root"),
  ]);
  assert.equal(tree[0].session.id, "root");
  assert.deepEqual(
    tree[0].children.map((node) => node.session.id),
    ["child", "sibling"],
  );
  assert.equal(tree[0].children[0].children[0].session.id, "grandchild");
});

test("cycles terminate and every branch is rendered exactly once", () => {
  const tree = buildSessionTree([
    session("a", 2, "b"),
    session("b", 1, "a"),
  ]);
  assert.deepEqual(ids(tree).sort(), ["a", "b"]);
});

test("duplicate ids and self-parenting do not duplicate nodes", () => {
  const tree = buildSessionTree([
    session("a", 2),
    session("a", 1),
    session("self", 1, "self"),
  ]);
  assert.deepEqual(ids(tree), ["a", "self"]);
});

test("graph joins sibling forks at their source and ends before the next root", () => {
  const graph = buildSessionGraph(buildSessionTree([
    session("fork-a", 4, "source"),
    session("fork-b", 3, "source"),
    session("source", 2),
    session("other", 1),
  ]));
  assert.deepEqual(graph.map((row) => row.node.session.id), ["fork-a", "fork-b", "source", "other"]);
  assert.deepEqual(graph[2].input.map((lane) => lane.targetId), ["source", "source"]);
  assert.equal(graph[2].output.length, 0);
  assert.equal(graph[3].input.length, 0);
  assert.equal(graph[3].output.length, 0);
});

test("graph carries nested forks to their actual parent and breaks corrupt cycles", () => {
  const graph = buildSessionGraph(buildSessionTree([
    session("grandchild", 5, "child"),
    session("child", 4, "root"),
    session("sibling", 3, "root"),
    session("root", 2),
    session("a", 1, "b"),
    session("b", 0, "a"),
  ]));
  assert.deepEqual(graph.map((row) => row.node.session.id), ["grandchild", "child", "sibling", "root", "b", "a"]);
  assert.equal(graph[0].output[0].targetId, "child");
  assert.equal(graph[1].output[0].targetId, "root");
  assert.equal(graph[3].output.length, 0);
  assert.equal(graph.at(-1)?.output.length, 0);
});

test("graph keeps known forks together when their source is outside the list", () => {
  const archived = [
    session("nested", 3, "fork"),
    session("fork", 2, "active-source"),
    session("independent", 1),
  ];
  const graph = buildSessionGraph(buildSessionTree(archived));
  assert.deepEqual(graph.map((row) => row.node.session.id), ["nested", "fork", "independent"]);
  assert.equal(graph[0].output[0].targetId, "fork");
  assert.equal(graph[1].node.missingParentId, "active-source");
  assert.equal(graph[1].output.length, 0);
  assert.equal(graph[2].input.length, 0);

  const afterRestore = buildSessionGraph(buildSessionTree(archived.filter((item) => item.id !== "fork")));
  assert.equal(afterRestore[0].node.missingParentId, "fork");
  assert.equal(afterRestore[0].output.length, 0);
});
