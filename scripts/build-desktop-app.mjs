import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { desktopSigningPlan } from "./desktop-signing.mjs";
import { bundleRuntime } from "./bundle-reset-runtime.mjs";
import { bundleWindowsRuntime } from "./bundle-windows-runtime.mjs";
import { captureSource, requireBuildSource, restoreManifestLineEndings, sourceEnvironment,
  verifyBuildSource, writeBuildProvenance } from "./build-provenance.mjs";

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
const cargoArguments = tauriArgs.indexOf("--");
const tauriOptions = cargoArguments < 0 ? tauriArgs : tauriArgs.slice(0, cargoArguments);
const targetIndex = tauriOptions.indexOf("--target");
const target = targetIndex >= 0 ? tauriOptions[targetIndex + 1]
  : tauriOptions.find((arg) => arg.startsWith("--target="))?.slice("--target=".length);
const source = captureSource(repositoryRoot);
if (process.env.GITHUB_ACTIONS === "true") requireBuildSource(source, process.env.GITHUB_SHA);
// A non-secret build fact, not a system environment change or signing identity.
const buildEnvironment = { ...process.env, ...sourceEnvironment(source), VITE_HORIZON_SIGNING_MODE: signing.mode };

if (!existsSync(tauriCli)) {
  fail("Tauri CLI is unavailable. Run `npm ci` or `npm install` first.");
}

if (process.platform === "darwin" && (!target || target === "aarch64-apple-darwin")) {
  const runtime = bundleRuntime(process.env.QUOTAHORIZON_PYTHON);
  tauriArgs.splice(cargoArguments < 0 ? tauriArgs.length : cargoArguments, 0,
    "--config", JSON.stringify({ bundle: { resources: { [`${runtime}/`]: "reset-runtime/" } } }));
}
if (process.platform === "win32") {
  if (target && target !== "x86_64-pc-windows-msvc") {
    fail("The complete Windows installer currently requires the x86_64-pc-windows-msvc target.");
  }
  const runtime = bundleWindowsRuntime();
  tauriArgs.splice(cargoArguments < 0 ? tauriArgs.length : cargoArguments, 0,
    "--config", JSON.stringify({ bundle: { resources: { [`${runtime}/`]: "reset-runtime/" } } }));
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
  ...(target ? ["--target", target] : []),
]);

if (signing.mode === "ad_hoc") {
  console.log("Local ad-hoc macOS build: upgrades can require renewed history Keychain authorization. A stable signing certificate is not configured.");
}

try {
  run(process.execPath, [tauriCli, "build", ...tauriArgs], desktopRoot);
} finally {
  restoreManifestLineEndings(source);
  verifyBuildSource(source);
}

// Tauri's `--no-sign` also skips the macOS bundle seal. A local daily-driver
// build still needs a complete ad-hoc seal so macOS verifies the bundle's
// resources consistently. This never replaces a configured distribution
// signature because it runs only for explicitly unsigned builds.
if (signing.sealAdHoc) {
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

verifyBuildSource(source);
mkdirSync(join(repositoryRoot, "dist"), { recursive: true });
writeBuildProvenance(join(repositoryRoot, "dist/build-provenance.json"), source);

function run(command, args, cwd = repositoryRoot) {
  const result = spawnSync(command, args, {
    cwd,
    env: buildEnvironment,
    stdio: "inherit",
  });
  if (result.error) {
    throw new Error(`Unable to run ${command}: ${result.error.message}`);
  }
  if ((result.status ?? 1) !== 0) {
    throw new Error(`${command} exited with status ${result.status ?? 1}`);
  }
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
