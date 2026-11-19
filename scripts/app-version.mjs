import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const semver = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

export function agreedVersion(versions) {
  const values = Object.values(versions);
  const unique = new Set(values);
  if (values.some((value) => !value) || unique.size !== 1) {
    const lines = Object.entries(versions).map(([name, value]) => `${name}=${value ?? ""}`);
    throw new Error(`版本号不一致:\n${lines.join("\n")}`);
  }
  const version = values[0];
  if (!semver.test(version)) {
    throw new Error(`版本号需要是主.次.修订，当前是 ${version}`);
  }
  return version;
}

export function readAppVersion(root) {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  const tauri = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
  const cargo = readFileSync(join(root, "Cargo.toml"), "utf8");
  const workspace = cargo.match(/\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  return agreedVersion({
    "package.json": pkg.version,
    "package-lock.json": lock.version,
    "package-lock.json#packages": lock.packages?.[""]?.version,
    "src-tauri/tauri.conf.json": tauri.version,
    "Cargo.toml": workspace?.[1],
  });
}

const entry = process.argv[1]?.replaceAll("\\", "/");
if (entry?.endsWith("scripts/app-version.mjs")) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  process.stdout.write(`${readAppVersion(root)}\n`);
}
