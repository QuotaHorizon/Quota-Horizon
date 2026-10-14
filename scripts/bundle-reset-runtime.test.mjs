import assert from "node:assert/strict";
import { test } from "node:test";
import { chmodSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { normalizeRuntimePermissions } from "./bundle-reset-runtime.mjs";

test("runtime packaging normalizes writable Conda cache modes without changing source files or link targets", {
  skip: process.platform === "win32" ? "macOS runtime packaging uses POSIX permissions" : false,
}, () => {
  const work = mkdtempSync(join(tmpdir(), "quotahorizon-runtime-modes-"));
  try {
    const source = join(work, "source");
    mkdirSync(join(source, "bin"), { recursive: true });
    writeFileSync(join(source, "module.py"), "runtime data\n");
    writeFileSync(join(source, "bin/python3"), "executable fixture\n");
    chmodSync(source, 0o775);
    chmodSync(join(source, "module.py"), 0o664);
    chmodSync(join(source, "bin/python3"), 0o775);
    const runtime = join(work, "runtime");
    cpSync(source, runtime, { recursive: true });
    symlinkSync(join(source, "module.py"), join(runtime, "linked.py"));
    const mode = path => statSync(path).mode & 0o777;

    normalizeRuntimePermissions(runtime);
    assert.equal(mode(runtime), 0o755);
    assert.equal(mode(join(runtime, "bin")), 0o755);
    assert.equal(mode(join(runtime, "module.py")), 0o644);
    assert.equal(mode(join(runtime, "bin/python3")), 0o755);
    assert.equal(readFileSync(join(runtime, "module.py"), "utf8"), "runtime data\n");
    assert.equal(mode(source), 0o775);
    assert.equal(mode(join(source, "module.py")), 0o664);
    assert.equal(mode(join(source, "bin/python3")), 0o775);
    assert.equal(mode(join(runtime, "linked.py")), 0o664);
    assert.throws(() => normalizeRuntimePermissions(join(runtime, "linked.py")), /real directory/);
  } finally {
    rmSync(work, { recursive: true });
  }
});
