import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const readJson = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));

test("macOS packaging dependencies do not block Windows npm ci", () => {
  const desktop = readJson("apps/desktop/package.json");
  const lock = readJson("apps/desktop/package-lock.json");
  assert.equal(desktop.dependencies["create-dmg"], undefined);
  assert.equal(desktop.devDependencies["create-dmg"], undefined);
  assert.equal(desktop.optionalDependencies["create-dmg"], "8.1.0");
  for (const name of ["create-dmg", "appdmg", "macos-alias", "fs-xattr"]) {
    assert.equal(lock.packages[`node_modules/${name}`].optional, true, name);
  }
});

test("Windows uses an installer target without changing the macOS bundle", () => {
  const base = readJson("apps/desktop/src-tauri/tauri.conf.json");
  const windows = readJson("apps/desktop/src-tauri/tauri.windows.conf.json");
  assert.deepEqual(base.bundle.targets, ["app"]);
  assert.deepEqual(windows.bundle.targets, ["nsis"]);
  assert.ok(windows.bundle.icon.includes("icons/icon.ico"));
  const smoke = readJson("apps/capacity-preview/src-tauri/tauri.conf.json");
  assert.ok(smoke.bundle.icon.includes("../../desktop/src-tauri/icons/icon.ico"));
  const icon = readFileSync(join(root, "apps/desktop/src-tauri/icons/icon.ico"));
  assert.equal(icon.readUInt16LE(0), 0);
  assert.equal(icon.readUInt16LE(2), 1);
  const frames = icon.readUInt16LE(4);
  assert.ok(frames > 0);
  for (let frame = 0; frame < frames; frame++) {
    const entry = 6 + frame * 16;
    const size = icon.readUInt32LE(entry + 8);
    const offset = icon.readUInt32LE(entry + 12);
    assert.ok(size > 0 && offset >= 6 + frames * 16 && offset + size <= icon.length);
  }
});

test("Windows shortcuts use native MSVC targets", () => {
  const { scripts } = readJson("apps/desktop/package.json");
  assert.match(scripts["build:app:win"], /--target x86_64-pc-windows-msvc$/);
  assert.match(scripts["build:app:win-arm64"], /--target aarch64-pc-windows-msvc$/);
});

test("source checks accept native paths against Git's portable file list", () => {
  const output = execFileSync(process.execPath, [join(root, "scripts/check-source.mjs")], {
    cwd: root,
    encoding: "utf8",
  });
  assert.match(output, /Source checks passed:/);
});
