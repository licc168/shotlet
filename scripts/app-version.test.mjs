import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { agreedVersion, readAppVersion } from "./app-version.mjs";

test("agreed version requires one semver", () => {
  assert.equal(agreedVersion({ a: "0.1.1", b: "0.1.1" }), "0.1.1");
  assert.equal(agreedVersion({ a: "1.2.3-beta.1", b: "1.2.3-beta.1" }), "1.2.3-beta.1");
  assert.throws(() => agreedVersion({ a: "0.1.1", b: "0.1.2" }), /版本号不一致/);
  assert.throws(() => agreedVersion({ a: "0.1", b: "0.1" }), /主\.次\.修订/);
});

test("repo manifests share one version", () => {
  const version = readAppVersion(fileURLToPath(new URL("..", import.meta.url)));
  assert.match(version, /^\d+\.\d+\.\d+/);
});
