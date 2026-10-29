import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { captureSource, requireBuildSource, restoreManifestLineEndings, sourceEnvironment,
  verifyBuildProvenance, verifyBuildSource, writeBuildProvenance } from "./build-provenance.mjs";

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "horizon-build-source-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = args => execFileSync("git", ["-c", "core.autocrlf=false", "-c", "core.hooksPath=", ...args], {
    cwd: root, encoding: "utf8", windowsHide: true, stdio: ["ignore", "pipe", "pipe"],
  }).trim();
  mkdirSync(join(root, "apps/desktop/src-tauri"), { recursive: true });
  mkdirSync(join(root, "dist"));
  const manifest = join(root, "apps/desktop/src-tauri/Cargo.toml");
  writeFileSync(manifest, '[package]\r\nname = "fixture"\r\nversion = "1.0.0"\r\n');
  writeFileSync(join(root, "source.txt"), "published source\n");
  writeFileSync(join(root, ".gitignore"), "dist/\nprivate/\n");
  git(["init", "-q"]);
  git(["add", "."]);
  const commit = () => git(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
    "-c", "commit.gpgsign=false", "commit", "-qm", "Record fixture inputs\n\n- Capture synthetic source for build verification"]);
  commit();
  return { root, manifest, git, commit };
}

test("build provenance preserves genuine local edits and requires clean CI inputs", t => {
  const { root } = fixture(t);
  const clean = captureSource(root);
  assert.equal(clean.dirty, false);
  requireBuildSource(clean, clean.commit);
  assert.throws(() => requireBuildSource(clean, "f".repeat(40)), /requested CI commit/);
  writeFileSync(join(root, "source.txt"), "local source\n");
  const modified = captureSource(root);
  assert.equal(modified.dirty, true);
  assert.equal(sourceEnvironment(modified).QUOTAHORIZON_BUILD_DIRTY, "true");
  assert.throws(() => requireBuildSource(modified, clean.commit), /local source changes/);
  verifyBuildSource(modified);
  writeFileSync(join(root, "new-source.txt"), "untracked source\n");
  assert.throws(() => verifyBuildSource(modified), /new-source.txt/);
});

test("Tauri newline normalization is restored without concealing manifest edits", t => {
  const { root, manifest } = fixture(t);
  // A user's unpublished manifest changes must also survive the restoration.
  writeFileSync(manifest, readFileSync(manifest, "utf8").replace("1.0.0", "1.1.0"));
  const before = captureSource(root);
  const bytes = readFileSync(manifest);
  writeFileSync(manifest, bytes.toString().replaceAll("\r\n", "\n"));
  restoreManifestLineEndings(before);
  assert.deepEqual(readFileSync(manifest), bytes);
  verifyBuildSource(before);
  assert.equal(before.dirty, true);
  writeFileSync(manifest, bytes.toString().replace("1.1.0", "1.2.0"));
  restoreManifestLineEndings(before);
  assert.match(readFileSync(manifest, "utf8"), /1\.2\.0/);
  assert.throws(() => verifyBuildSource(before), /Cargo.toml/);
});

test("provenance verification detects source and commit changes after build", t => {
  const { root, git, commit } = fixture(t);
  const source = captureSource(root);
  const record = join(root, "dist/build-provenance.json");
  writeBuildProvenance(record, source);
  assert.equal(verifyBuildProvenance(root, source.commit, true).dirty, false);
  writeFileSync(join(root, "source.txt"), "changed during compilation\n");
  assert.throws(() => verifyBuildProvenance(root, source.commit, true), /build inputs differ/);
  git(["add", "source.txt"]);
  commit();
  assert.throws(() => verifyBuildSource(source), /commit changed/);
});

test("source archives do not inherit an enclosing repository identity", t => {
  const { root } = fixture(t);
  const archive = join(root, "dist/archive");
  mkdirSync(archive);
  assert.equal(captureSource(archive), null);
  assert.deepEqual(sourceEnvironment(null), {
    QUOTAHORIZON_BUILD_COMMIT: "unknown", QUOTAHORIZON_BUILD_DIRTY: "unknown",
    QUOTAHORIZON_BUILD_INPUT_DIGEST: "unknown",
  });
  assert.throws(() => requireBuildSource(null, "f".repeat(40)), /Git source checkout/);
});

test("source archives remain buildable when Git is unavailable", t => {
  const { root } = fixture(t);
  const module = new URL("./build-provenance.mjs", import.meta.url).href;
  const result = execFileSync(process.execPath, ["--input-type=module", "-e",
    `import { captureSource } from ${JSON.stringify(module)}; console.log(JSON.stringify(captureSource(process.argv[1])));`, root], {
    env: { ...process.env, PATH: "" }, encoding: "utf8", windowsHide: true,
  });
  assert.equal(result.trim(), "null");
});
