import assert from "node:assert/strict";
import { test } from "node:test";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { bundleInventory, createDmgArguments, packageOptions, releaseFilename, verifyMetadata, verifySystemLibraries } from "./package-macos.mjs";

const config = {
  identifier: "io.github.halfmelon.quotahorizon", productName: "QuotaHorizon", version: "0.1.4",
  bundle: { macOS: { minimumSystemVersion: "13.0" } },
};
const info = {
  CFBundleIdentifier: config.identifier, CFBundleName: config.productName,
  CFBundleShortVersionString: config.version, CFBundleVersion: config.version,
  CFBundleExecutable: "quota-horizon-desktop", LSMinimumSystemVersion: "13.0",
};

test("packaging builds by default and supports an explicit existing-build option", () => {
  assert.deepEqual(packageOptions([]), { skipBuild: false, help: false });
  assert.equal(packageOptions(["--skip-build"]).skipBuild, true);
  assert.equal(packageOptions(["--help"]).help, true);
  assert.throws(() => packageOptions(["--skip-bulid"]), /Unknown option/);
});

test("the ready-made DMG template receives literal app and output paths", () => {
  assert.deepEqual(createDmgArguments("/tmp/App With Spaces.app", "/tmp/installer output"), [
    "/tmp/App With Spaces.app", "/tmp/installer output", "--no-code-sign", "--no-version-in-filename",
  ]);
});

test("release names identify the version, platform and architecture", () => {
  assert.equal(releaseFilename("0.1.4"), "QuotaHorizon_0.1.4_macos-arm64.dmg");
  assert.equal(releaseFilename("1.0.0-beta.1"), "QuotaHorizon_1.0.0-beta.1_macos-arm64.dmg");
  for (const invalid of ["../0.1.4", "", "0.1", "0.1.4/extra"]) {
    assert.throws(() => releaseFilename(invalid));
  }
});

test("bundle metadata and the actual architecture must match the release", () => {
  verifyMetadata(info, config, "arm64\n");
  for (const field of Object.keys(info)) {
    assert.throws(() => verifyMetadata({ ...info, [field]: "different" }, config, "arm64"));
  }
  for (const architecture of ["x86_64", "x86_64 arm64", ""]) {
    assert.throws(() => verifyMetadata(info, config, architecture));
  }
});

test("a distributable executable uses only system libraries", () => {
  const header = "/tmp/QuotaHorizon.app/Contents/MacOS/quota-horizon-desktop:\n";
  verifySystemLibraries(`${header}\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)\n\t/System/Library/Frameworks/WebKit.framework/Versions/A/WebKit (compatibility version 1.0.0, current version 1.0.0)\n`);
  for (const library of ["/opt/homebrew/lib/libssl.dylib", "/usr/local/lib/test.dylib", "@rpath/test.dylib"]) {
    assert.throws(() => verifySystemLibraries(`${header}\t${library} (compatibility version 1.0.0)`), /External runtime/);
  }
  assert.throws(() => verifySystemLibraries(header), /no reported system libraries/);
});

test("bundle comparison covers nested files, executable permissions and symlinks", () => {
  const work = mkdtempSync(join(tmpdir(), "quotahorizon-package-test-"));
  try {
    const app = join(work, "Test.app");
    mkdirSync(join(app, "Contents"), { recursive: true });
    const executable = join(app, "Contents", "executable");
    writeFileSync(executable, "synthetic executable", { mode: 0o755 });
    symlinkSync("Contents/executable", join(app, "link"));
    const original = bundleInventory(app);
    assert.deepEqual(bundleInventory(app), original);
    assert.ok(original.some((entry) => entry[1] === "link" && entry[2] === "Contents/executable"));
    chmodSync(executable, 0o644);
    assert.notDeepEqual(bundleInventory(app), original);
    chmodSync(executable, 0o755);
    writeFileSync(executable, "changed executable");
    assert.notDeepEqual(bundleInventory(app), original);
  } finally {
    rmSync(work, { recursive: true });
  }
});
