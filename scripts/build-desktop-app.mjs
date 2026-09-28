import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { desktopSigningPlan } from "./desktop-signing.mjs";

// Modified by the QuotaHorizon project from Codex Switch v1.3.5.

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(scriptDirectory, "..");
const desktopRoot = join(repositoryRoot, "apps", "desktop");
const tauriRoot = join(desktopRoot, "src-tauri");
const cargoManifest = join(tauriRoot, "Cargo.toml");
const tauriCli = join(
  desktopRoot,
  "node_modules",
  "@tauri-apps",
  "cli",
  "tauri.js",
);
const config = JSON.parse(readFileSync(join(tauriRoot, "tauri.conf.json"), "utf8"));
const signing = desktopSigningPlan({ platform: process.platform, args: process.argv.slice(2),
  config, environment: process.env });
const tauriArgs = signing.args;
// A non-secret build fact, not a system environment change or signing identity.
const buildEnvironment = { ...process.env, VITE_HORIZON_SIGNING_MODE: signing.mode };

if (!existsSync(tauriCli)) {
  fail("Tauri CLI is unavailable. Run `npm ci` or `npm install` first.");
}

// A normal `cargo build --release` and `tauri build` use different Tauri
// features but write to the same top-level executable. Cargo can regard the
// custom-protocol artifact as fresh while the top-level executable was most
// recently overwritten by the normal build. Cleaning this package's release
// artifacts forces Tauri to relink the executable without recompiling every
// dependency.
console.log("Preparing a clean Tauri release executable...");
run("cargo", [
  "clean",
  "--manifest-path",
  cargoManifest,
  "--release",
  "--package",
  "quota-horizon-desktop",
]);

if (signing.mode === "ad_hoc") {
  console.log("Local ad-hoc macOS build: upgrades can require renewed history Keychain authorization. A stable signing certificate is not configured.");
}

run(process.execPath, [tauriCli, "build", ...tauriArgs], desktopRoot);

// Tauri's `--no-sign` also skips the macOS bundle seal. A local daily-driver
// build still needs a complete ad-hoc seal so macOS verifies the bundle's
// resources consistently. This never replaces a configured distribution
// signature because it runs only for explicitly unsigned builds.
if (signing.sealAdHoc) {
  const targetIndex = tauriArgs.indexOf("--target");
  const target = targetIndex >= 0 ? tauriArgs[targetIndex + 1] : null;
  const targetRoot = target
    ? join(tauriRoot, "target", target, "release")
    : join(tauriRoot, "target", "release");
  const appBundle = join(targetRoot, "bundle", "macos", "QuotaHorizon.app");
  if (!existsSync(appBundle)) {
    fail(`Expected macOS application bundle is unavailable: ${appBundle}`);
  }
  console.log("Applying an ad-hoc signature to the local macOS bundle...");
  run("/usr/bin/codesign", ["--force", "--deep", "--sign", "-", appBundle]);
  run("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", appBundle]);
}

function run(command, args, cwd = repositoryRoot) {
  const result = spawnSync(command, args, {
    cwd,
    env: buildEnvironment,
    stdio: "inherit",
  });
  if (result.error) {
    fail(`Unable to run ${command}: ${result.error.message}`);
  }
  if ((result.status ?? 1) !== 0) {
    process.exit(result.status ?? 1);
  }
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
