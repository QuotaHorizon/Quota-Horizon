import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync,
  readdirSync, readlinkSync, rmSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = "aarch64-apple-darwin";

export function createDmgArguments(app, destination) {
  return [app, destination, "--no-code-sign", "--no-version-in-filename"];
}

export function packageOptions(args) {
  const unknown = args.filter((arg) => !["--skip-build", "--help"].includes(arg));
  if (unknown.length) throw new Error(`Unknown option: ${unknown.join(" ")}`);
  return { skipBuild: args.includes("--skip-build"), help: args.includes("--help") };
}

export function releaseFilename(version) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error("The desktop version must be a version number.");
  }
  return `QuotaHorizon_${version}_macos-arm64.dmg`;
}

export function verifyMetadata(info, config, architectures) {
  assert.equal(info.CFBundleIdentifier, config.identifier, "Application identifier differs");
  assert.equal(info.CFBundleName, config.productName, "Application name differs");
  assert.equal(info.CFBundleShortVersionString, config.version, "Application version differs");
  assert.equal(info.CFBundleVersion, config.version, "Application build version differs");
  assert.equal(info.CFBundleExecutable, "quota-horizon-desktop", "Unexpected executable");
  assert.equal(info.LSMinimumSystemVersion, config.bundle.macOS.minimumSystemVersion,
    "Minimum macOS version differs");
  assert.deepEqual(architectures.trim().split(/\s+/).sort(), ["arm64"],
    "The macos-arm64 package requires an arm64 executable");
}

export function verifySystemLibraries(output) {
  const libraries = output.trim().split("\n").slice(1).map((line) =>
    line.trim().split(" (compatibility version")[0]);
  assert.ok(libraries.length > 0, "Executable has no reported system libraries");
  for (const library of libraries) {
    assert.ok(library.startsWith("/System/Library/") || library.startsWith("/usr/lib/"),
      `External runtime dependency: ${library}`);
  }
}

// Compare every packaged file, link and permission bit after the install copy.
export function bundleInventory(directory) {
  const entries = [];
  function visit(relative = "") {
    for (const name of readdirSync(join(directory, relative)).sort()) {
      const path = join(relative, name);
      const absolute = join(directory, path);
      const stat = lstatSync(absolute);
      if (stat.isSymbolicLink()) {
        entries.push([path, "link", readlinkSync(absolute)]);
      } else if (stat.isDirectory()) {
        entries.push([path, "directory", stat.mode & 0o777]);
        visit(path);
      } else if (stat.isFile()) {
        entries.push([path, "file", stat.mode & 0o777, sha256(absolute)]);
      } else {
        throw new Error(`Unexpected bundle entry: ${path}`);
      }
    }
  }
  visit();
  return entries;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function run(command, args, cwd = root) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status ?? result.signal}`);
}

function inspectBundle(app, config) {
  const info = JSON.parse(execFileSync("/usr/bin/plutil", [
    "-convert", "json", "-o", "-", join(app, "Contents", "Info.plist"),
  ], { encoding: "utf8" }));
  const executable = join(app, "Contents", "MacOS", "quota-horizon-desktop");
  verifyMetadata(info, config,
    execFileSync("/usr/bin/lipo", ["-archs", executable], { encoding: "utf8" }));
  verifySystemLibraries(execFileSync("/usr/bin/otool", ["-L", executable], { encoding: "utf8" }));
  run("/usr/bin/codesign", ["--verify", "--deep", "--strict", app]);
  for (const name of ["LICENSE", "NOTICE"]) {
    assert.equal(sha256(join(app, "Contents", "Resources", name)), sha256(join(root, name)),
      `Bundled ${name} differs from source`);
  }
}

function main() {
  const options = packageOptions(process.argv.slice(2));
  if (options.help) {
    console.log("Usage: npm run package:mac -- [--skip-build]\nBuild and verify an Apple Silicon DMG. --skip-build packages the existing release app.");
    return;
  }
  assert.equal(process.platform, "darwin", "Run macOS packaging on a Mac");
  const config = JSON.parse(readFileSync(join(root, "apps/desktop/src-tauri/tauri.conf.json"), "utf8"));
  const desktop = JSON.parse(readFileSync(join(root, "apps/desktop/package.json"), "utf8"));
  assert.equal(config.version, desktop.version, "Desktop package versions differ");
  // Keep outputs versioned and refuse to replace an already prepared release.
  const output = join(root, "dist", "releases", config.version);
  if (existsSync(output)) throw new Error(`Release directory already exists: ${output}`);
  const filename = releaseFilename(config.version);
  const createDmg = join(root, "apps/desktop/node_modules/create-dmg/cli.js");
  if (!existsSync(createDmg)) {
    throw new Error("The macOS packaging tool is missing. Run `npm --prefix apps/desktop ci --no-audit --no-fund`.");
  }
  if (!options.skipBuild) {
    run(process.execPath, [join(root, "scripts/build-desktop-app.mjs"), "--target", target]);
  }
  const app = join(root, "apps/desktop/src-tauri/target", target,
    "release/bundle/macos/QuotaHorizon.app");
  inspectBundle(app, config);
  const expected = bundleInventory(app);

  // Package only the built app. User data and the source checkout stay outside it.
  const work = mkdtempSync(join(tmpdir(), "quotahorizon-dmg-"));
  const mount = join(work, "mounted");
  const image = join(work, `${config.productName}.dmg`);
  let mounted = false;
  try {
    mkdirSync(mount);
    // Use create-dmg's complete installer template: background, arrow, Retina
    // assets, large icons and fixed window layout. Keep the app bundle intact.
    run(process.execPath, [createDmg, ...createDmgArguments(app, work)], work);
    run("/usr/bin/hdiutil", ["verify", image]);
    run("/usr/bin/hdiutil", ["attach", "-readonly", "-nobrowse", "-noautoopen",
      "-mountpoint", mount, image]);
    mounted = true;
    assert.ok(existsSync(join(mount, ".DS_Store")), "Installer window layout is missing");
    assert.ok(readdirSync(join(mount, ".background")).length > 0,
      "Installer background is missing");
    assert.equal(readlinkSync(join(mount, "Applications")), "/Applications");
    assert.deepEqual(readdirSync(mount).filter((name) => !name.startsWith(".")).sort(),
      ["Applications", "QuotaHorizon.app"]);

    const installed = join(work, "installed", "QuotaHorizon.app");
    run("/usr/bin/ditto", [join(mount, "QuotaHorizon.app"), installed]);
    inspectBundle(installed, config);
    assert.deepEqual(bundleInventory(installed), expected, "Installed copy differs from the built app");
    run("/usr/bin/hdiutil", ["detach", mount]);
    mounted = false;

    const checksum = `${sha256(image)}  ${filename}\n`;
    mkdirSync(output, { recursive: true });
    copyFileSync(image, join(output, filename));
    writeFileSync(join(output, "SHA256SUMS"), checksum, { flag: "wx" });
    console.log(`\nReady: ${join(output, filename)}\nChecksums: ${join(output, "SHA256SUMS")}`);
    console.log("Verified the disk image, install copy, arm64 executable, resources and system-only runtime dependencies.");
  } finally {
    // Never recurse through an attached volume (which includes /Applications).
    try {
      if (mounted) run("/usr/bin/hdiutil", ["detach", mount]);
      rmSync(work, { recursive: true });
    } catch (error) {
      console.error(`Temporary packaging files retained at ${work}: ${error.message}`);
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) {
    console.error(`macOS packaging failed: ${error.message}`);
    process.exitCode = 1;
  }
}
