import assert from "node:assert/strict";
import test from "node:test";
import { changeConversation, classifySend, conversation, moveDraft, promptError } from "../mods/herdr-queue/queue-state.js";

test("prompt validation matches the native queue contract", () => {
  assert.match(promptError("  \n"), /Écris/);
  assert.match(promptError("hello\u001b[31m"), /contrôle/);
  assert.equal(promptError("é".repeat(32768)), null);
  assert.match(promptError("é".repeat(32769)), /64 Kio/);
  assert.equal(promptError("Bonjour\nmerci 🎯"), null);
});

test("conversations stay isolated and retain unfinished text", () => {
  const first = changeConversation(null, "t1", state => {
    state.composer = "unfinished";
    state.drafts.push({ id: "one", body: "next", state: "draft" });
  });
  const second = changeConversation(first, "t2", state => { state.composer = "other"; });
  assert.equal(conversation(second, "t1").composer, "unfinished");
  assert.deepEqual(conversation(second, "t1").drafts.map(item => item.id), ["one"]);
  assert.equal(conversation(second, "t2").composer, "other");
  assert.equal(conversation(first, "t2").composer, "");
});

test("reordering does not mutate the saved queue", () => {
  const original = [{ id: "a" }, { id: "b" }, { id: "c" }];
  const reordered = moveDraft(original, "c", -1);
  assert.deepEqual(reordered.map(item => item.id), ["a", "c", "b"]);
  assert.deepEqual(original.map(item => item.id), ["a", "b", "c"]);
  assert.equal(moveDraft(original, "a", -1), original);
});

test("uncertain delivery is distinct from a known refusal", () => {
  assert.equal(classifySend({ code: 0, stderr: "" }).state, "sent");
  assert.equal(classifySend({ code: 9, stderr: '{"error":{"kind":"blocked","message":"Blocked"}}' }).state, "error");
  assert.equal(classifySend({ code: 5, stderr: '{"error":{"kind":"unavailable","message":"Disconnected"}}' }).state, "uncertain");
  assert.equal(classifySend({ code: 1, stderr: "connection lost" }).state, "uncertain");
});
