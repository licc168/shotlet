import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const SHELL_PIN = new Set(["pin: command", "pin: plain command", "pin: annotated command"]);

/** One F1 press or one pin, in log order. */
export function summarize(text) {
  const marks = [];
  for (const line of text.split(/\r?\n/)) {
    const match = line.match(/^\[trace (\d+)\] (.+)$/);
    if (!match) continue;
    marks.push({ at: BigInt(match[1]), stage: match[2] });
  }
  const groups = [];
  for (const mark of marks) {
    if (isStart(mark.stage)) {
      const shell = SHELL_PIN.has(mark.stage);
      const open = groups.at(-1);
      if (shell && open?.kind === "pin" && !open.marks.some((item) => SHELL_PIN.has(item.stage))) {
        open.marks.push(mark);
        continue;
      }
      groups.push({ kind: mark.stage.startsWith("f1:") ? "f1" : "pin", marks: [mark] });
      continue;
    }
    const open = groups.at(-1);
    const kind = mark.stage.startsWith("f1:") || mark.stage.startsWith("capture:") ? "f1" : mark.stage.startsWith("pin:") ? "pin" : null;
    if (open && kind === open.kind) open.marks.push(mark);
  }
  return groups.map((group) => ({
    kind: group.kind,
    marks: group.marks,
    gaps: gaps(group.marks),
  }));
}

function isStart(stage) {
  return stage === "f1: key" || stage === "pin: click" || stage === "pin: plain click" || stage === "pin: annotated png" || SHELL_PIN.has(stage);
}

function gaps(marks) {
  const rows = [];
  for (let index = 1; index < marks.length; index += 1) {
    const previous = marks[index - 1];
    const current = marks[index];
    const ms = Number(current.at - previous.at) / 1000;
    rows.push({ from: previous.stage, to: current.stage, ms });
  }
  return rows;
}

export function formatSummary(groups) {
  if (groups.length === 0) return "日志里没有 [trace 行。先设 SHOTLET_TRACE=1 再操作一次。\n";
  const lines = [];
  const counts = { f1: 0, pin: 0 };
  for (const group of groups) {
    counts[group.kind] += 1;
    const title = group.kind === "f1" ? `F1 #${counts.f1}` : `贴图 #${counts.pin}`;
    lines.push(title);
    const first = group.marks[0];
    lines.push(`  ${first.stage}`);
    for (const gap of group.gaps) {
      const cloaked = gap.ms >= 45 && gap.ms <= 55 && /painted|ready|revealed/.test(`${gap.from} ${gap.to}`);
      const note = cloaked ? "  （接近 48ms，cloak 下可能没有动画帧）" : "";
      lines.push(`  +${gap.ms.toFixed(1)} ms  ${gap.to}${note}`);
    }
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invoked) {
  const path = process.argv[2];
  if (!path) {
    console.error("usage: node scripts/trace-summary.mjs <shotlet.log>");
    process.exit(1);
  }
  process.stdout.write(formatSummary(summarize(readFileSync(path, "utf8"))));
}
