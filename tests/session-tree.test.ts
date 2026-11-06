import assert from "node:assert/strict";
import test from "node:test";
import { buildSessionTree, type SessionTreeNode } from "../src/chat/session-tree.ts";
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
