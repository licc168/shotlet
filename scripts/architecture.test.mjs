import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const manifests = {
  "shotlet-types": "crates/shotlet-types/Cargo.toml",
  "shotlet-platform": "crates/shotlet-platform/Cargo.toml",
  "shotlet-image": "crates/shotlet-image/Cargo.toml",
  "shotlet-capture": "crates/shotlet-capture/Cargo.toml",
  "shotlet-store": "crates/shotlet-store/Cargo.toml",
  "shotlet-media": "crates/shotlet-media/Cargo.toml",
  "shotlet-core": "crates/shotlet-core/Cargo.toml",
  shotlet: "src-tauri/Cargo.toml",
};

const allowed = {
  "shotlet-types": [],
  "shotlet-platform": [],
  "shotlet-image": [],
  "shotlet-capture": ["shotlet-types", "shotlet-platform"],
  "shotlet-store": ["shotlet-types", "shotlet-platform"],
  "shotlet-media": ["shotlet-capture", "shotlet-image", "shotlet-platform"],
  "shotlet-core": ["shotlet-capture", "shotlet-store", "shotlet-media", "shotlet-platform"],
  shotlet: ["shotlet-core"],
};

function workspaceDeps(toml) {
  const names = [];
  for (const line of toml.split(/\r?\n/)) {
    const match = line.match(/^([A-Za-z0-9_-]+)(?:\.workspace)?\s*=/);
    if (!match) continue;
    const name = match[1];
    if (name.startsWith("shotlet-") || name === "shotlet") names.push(name);
  }
  return names;
}

test("workspace crates only depend downward", () => {
  for (const [name, path] of Object.entries(manifests)) {
    const deps = workspaceDeps(readFileSync(path, "utf8")).sort();
    assert.deepEqual(deps, [...allowed[name]].sort(), path);
  }
});

test("core owns the procedures and commands only adapt them", () => {
  const commands = readFileSync("src-tauri/src/commands.rs", "utf8");
  for (const token of ["history::", "capture::", "scroll::", "record::", "ocr::", "util::", "settings::"]) {
    assert.equal(commands.includes(token), false, `commands.rs still names ${token}`);
  }
  for (const token of ["capture_session::", "library::", "scroll_session::", "record_session::"]) {
    assert.equal(commands.includes(token), true, `commands.rs does not call ${token}`);
  }
  const read = (name) => readFileSync(`crates/shotlet-core/src/${name}`, "utf8");
  assert.equal(read("capture_session.rs").includes("freeze_under_cursor"), true);
  assert.equal(read("library.rs").includes("history::commit"), true);
  assert.equal(read("library.rs").includes("crop_frozen"), true);
  assert.equal(read("scroll_session.rs").includes("capture_manual"), true);
  assert.equal(read("record_session.rs").includes("record::start"), true);
});
