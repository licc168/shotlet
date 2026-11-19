import assert from "node:assert/strict";
import test from "node:test";
import { formatSummary, summarize } from "./trace-summary.mjs";

test("one pin stays one group and the next F1 starts another", () => {
  const text = [
    "[trace 1000000] pin: standby built",
    "[trace 2000000] pin: plain click",
    "[trace 2001500] pin: plain command",
    "[trace 2005000] pin: revealed",
    "[trace 9000000] f1: key",
    "[trace 9002000] f1: present done",
    "noise",
  ].join("\n");
  const groups = summarize(text);
  assert.equal(groups.length, 2);
  assert.equal(groups[0].kind, "pin");
  assert.deepEqual(groups[0].marks.map((mark) => mark.stage), ["pin: plain click", "pin: plain command", "pin: revealed"]);
  assert.equal(groups[0].gaps[0].ms, 1.5);
  assert.equal(groups[1].kind, "f1");
  assert.equal(groups[1].gaps[0].ms, 2);
  const printed = formatSummary(groups);
  assert.match(printed, /贴图 #1/);
  assert.match(printed, /F1 #1/);
  assert.match(printed, /\+1\.5 ms/);
});

test("a painted-to-ready gap near 48ms is called out", () => {
  const text = ["[trace 0] pin: click", "[trace 48000] pin: painted"].join("\n");
  const printed = formatSummary(summarize(text));
  assert.match(printed, /48ms/);
});
