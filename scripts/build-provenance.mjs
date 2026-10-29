import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readlinkSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const manifestPath = "apps/desktop/src-tauri/Cargo.toml";
const hash = value => createHash("sha256").update(value).digest("hex");
const git = (root, args) => execFileSync("git", args, {
  cwd: root, encoding: "utf8", windowsHide: true,
  env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" }, stdio: ["ignore", "pipe", "pipe"],
});
const samePath = (left, right) => {
  // The JavaScript resolver preserves Windows 8.3 names; Git reports long
  // names. Native resolution expands both aliases before comparing roots.
  const canonical = value => process.platform === "win32" ? realpathSync.native(value).toLowerCase() : realpathSync(value);
  return canonical(left) === canonical(right);
};

export function captureSource(root) {
  root = resolve(root);
  let commit = null, dirty = null, paths;
  try {
    if (!samePath(root, git(root, ["rev-parse", "--show-toplevel"]).trim())) return null;
    commit = git(root, ["rev-parse", "HEAD"]).trim();
    assert.match(commit, /^[a-f0-9]{40}$/);
    dirty = git(root, ["status", "--porcelain=v1", "--untracked-files=normal"]).trim().length > 0;
    paths = [...new Set(git(root, ["ls-files", "-z", "--cached", "--others", "--exclude-standard"]).split("\0").filter(Boolean))].sort();
  } catch (error) {
    if (error.status || error.code === "ENOENT") return null;
    throw error;
  }
  const inputs = paths.map(path => {
    const absolute = join(root, path);
    try {
      const stat = lstatSync(absolute);
      assert.ok(stat.isFile() || stat.isSymbolicLink(), `Unsupported build source: ${path}`);
      const bytes = stat.isSymbolicLink() ? Buffer.from(readlinkSync(absolute)) : readFileSync(absolute);
      return [path, stat.mode, hash(bytes)];
    } catch (error) {
      if (error.code === "ENOENT") return [path, "missing"];
      throw error;
    }
  });
  return { root, commit, dirty, inputs, inputDigest: hash(JSON.stringify(inputs)),
    manifest: readFileSync(join(root, manifestPath)) };
}

export function requireBuildSource(source, expectedCommit) {
  assert.ok(source, "CI build requires a Git source checkout");
  assert.equal(source.commit, expectedCommit, "Build checkout differs from the requested CI commit");
  assert.equal(source.dirty, false, "CI build has local source changes; inspect git status before packaging");
}

export function restoreManifestLineEndings(source) {
  if (!source) return;
  const path = join(source.root, manifestPath);
  const current = readFileSync(path);
  // Tauri CLI serializes Cargo.toml with LF even on a CRLF checkout. Restore
  // only that byte-equivalent newline conversion; feature/version edits fail.
  if (!current.equals(source.manifest)
      && current.toString("utf8").replaceAll("\r\n", "\n") === source.manifest.toString("utf8").replaceAll("\r\n", "\n")) {
    writeFileSync(path, source.manifest);
  }
}

export function verifyBuildSource(source) {
  if (!source) return;
  const after = captureSource(source.root);
  assert.ok(after, "Build source checkout disappeared");
  assert.equal(after.commit, source.commit, "Source commit changed during the build");
  if (after.inputDigest !== source.inputDigest) {
    const previous = new Map(source.inputs.map(row => [row[0], JSON.stringify(row)]));
    const current = new Map(after.inputs.map(row => [row[0], JSON.stringify(row)]));
    const changed = [...new Set([...previous.keys(), ...current.keys()])].filter(path => previous.get(path) !== current.get(path));
    throw new Error(`Build inputs changed during compilation: ${changed.join(", ")}`);
  }
}

export function sourceEnvironment(source) {
  return {
    QUOTAHORIZON_BUILD_COMMIT: source?.commit ?? "unknown",
    QUOTAHORIZON_BUILD_DIRTY: source ? String(source.dirty) : "unknown",
    QUOTAHORIZON_BUILD_INPUT_DIGEST: source?.inputDigest ?? "unknown",
  };
}

export function writeBuildProvenance(path, source) {
  writeFileSync(path, JSON.stringify({ schema: "horizon-build-source/1", commit: source?.commit ?? null,
    dirty: source?.dirty ?? null, inputDigest: source?.inputDigest ?? null }, null, 2) + "\n");
}

export function verifyBuildProvenance(root, expectedCommit, requireClean = false) {
  const record = JSON.parse(readFileSync(join(root, "dist/build-provenance.json"), "utf8"));
  const source = captureSource(root);
  assert.equal(record.schema, "horizon-build-source/1");
  assert.ok(source, "Build verification requires a Git checkout");
  assert.equal(record.commit, source.commit, "Recorded build commit differs from checkout");
  assert.equal(record.inputDigest, source.inputDigest, "Recorded build inputs differ from checkout");
  if (expectedCommit) assert.equal(record.commit, expectedCommit, "Build differs from the requested CI commit");
  if (requireClean) {
    requireBuildSource(source, expectedCommit);
    assert.equal(record.dirty, false, "Artifact was built from modified source");
  }
  return record;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  try {
    const record = verifyBuildProvenance(root, process.env.GITHUB_SHA, process.env.GITHUB_ACTIONS === "true");
    console.log(`Verified build source ${record.commit} (${record.dirty ? "local changes" : "clean"}).`);
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
