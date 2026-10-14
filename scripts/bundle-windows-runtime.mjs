import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { moduleDigest } from "./bundle-reset-runtime.mjs";
import { windowsRuntimeBase, windowsRuntimeLock } from "./prepare-windows-runtime.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const hash = data => createHash("sha256").update(data).digest("hex");
const inside = (parent, child) => {
  const path = relative(parent, child);
  return !isAbsolute(path) && path !== ".." && !path.startsWith("..\\") && !path.startsWith("../");
};

export function verifyWindowsRuntime(destination) {
  destination = realpathSync(destination);
  const manifest = JSON.parse(readFileSync(join(destination, "reset-runtime-manifest.json"), "utf8"));
  assert.equal(manifest.platform, "windows-x64");
  assert.equal(manifest.modules, moduleDigest(), "Bundled modules differ from source");
  function visit(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (/\.(dll|pyd|exe)$/i.test(entry.name)) {
        const bytes = readFileSync(path);
        const pe = bytes.readUInt32LE(0x3c);
        assert.equal(bytes.toString("ascii", pe, pe + 4), "PE\0\0", `Invalid PE: ${entry.name}`);
        assert.equal(bytes.readUInt16LE(pe + 4), 0x8664, `Non-x64 runtime binary: ${entry.name}`);
      }
    }
  }
  visit(destination);
  const check = JSON.parse(execFileSync(join(destination, "python.exe"), ["-I", "-B", "-X", "utf8", "-c",
    "import sys,ssl,sqlite3,numpy,reset_intelligence.__main__,reset_observer.__main__,json; from zoneinfo import ZoneInfo; ZoneInfo('America/Los_Angeles'); print(json.dumps({'prefix':sys.prefix,'numpy':numpy.__version__,'paths':sys.path,'inverse':numpy.linalg.inv(numpy.eye(2)).tolist(),'certs':ssl.create_default_context().cert_store_stats()['x509_ca']}))"], {
    cwd: process.env.SystemRoot,
    windowsHide: true,
    encoding: "utf8",
    env: { SystemRoot: process.env.SystemRoot, PATH: join(process.env.SystemRoot, "System32") },
  }));
  assert.equal(realpathSync(check.prefix), destination);
  assert.ok(check.paths.every(p => inside(destination, p)), "Runtime search path escapes the bundle");
  assert.equal(check.numpy, windowsRuntimeLock.numpy.version);
  assert.deepEqual(check.inverse, [[1, 0], [0, 1]]);
  assert.ok(check.certs > 0, "Windows certificate store is unavailable");
  return manifest;
}

export function bundleWindowsRuntime(destination = join(root, "dist/reset-runtime")) {
  if (process.platform !== "win32" || process.arch !== "x64") throw new Error("The Windows bundled runtime requires an x64 Windows build host.");
  if (!existsSync(join(windowsRuntimeBase, "python.exe"))) throw new Error("Run `node scripts/prepare-windows-runtime.mjs` to prepare the project-local runtime first.");
  assert.deepEqual(JSON.parse(readFileSync(join(windowsRuntimeBase, "windows-runtime-lock.json"), "utf8")), windowsRuntimeLock,
    "Prepared runtime differs from the lockfile; prepare it again");
  rmSync(destination, { recursive: true, force: true });
  cpSync(windowsRuntimeBase, destination, { recursive: true,
    filter: p => !["tests", "__pycache__"].includes(basename(p)) && !p.endsWith(".pyc") });
  const site = join(destination, "Lib/site-packages");
  for (const [directory, name] of [["reset-intelligence", "reset_intelligence"], ["reset-observer", "reset_observer"]]) {
    cpSync(join(root, "modules", directory, name), join(site, name), { recursive: true,
      filter: p => basename(p) !== "__pycache__" && !p.endsWith(".pyc") });
  }
  const manifest = { schema_version: "reset-runtime/1", platform: "windows-x64", python: "3.13",
    numpy: windowsRuntimeLock.numpy.version, modules: moduleDigest(), packages: windowsRuntimeLock };
  manifest.revision = hash(JSON.stringify(manifest));
  writeFileSync(join(destination, "reset-runtime-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  verifyWindowsRuntime(destination);
  console.log(`Verified isolated Windows forecasting runtime (${manifest.revision.slice(0, 12)}).`);
  return destination;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv[2] === "--verify") verifyWindowsRuntime(resolve(process.argv[3]));
    else bundleWindowsRuntime();
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
