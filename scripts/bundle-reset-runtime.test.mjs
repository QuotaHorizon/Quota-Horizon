import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { chmodSync, cpSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { bundleTimezoneData, copyRuntimeNotices, normalizeRuntimePermissions, timezoneDigest, verifyRuntimeTimezone } from "./bundle-reset-runtime.mjs";

function timezoneFixture(work) {
  const prefix = join(work, "source"), destination = join(work, "runtime"), site = join(destination, "lib/python3.13/site-packages");
  mkdirSync(join(prefix, "share/zoneinfo/America"), { recursive: true });
  mkdirSync(site, { recursive: true });
  writeFileSync(join(prefix, "share/zoneinfo/UTC"), "UTC fixture");
  writeFileSync(join(prefix, "share/zoneinfo/America/Los_Angeles"), "LA fixture");
  return { prefix, destination, site };
}

test("timezone data and startup are copied without changing source and affect the manifest revision", () => {
  const work = mkdtempSync(join(tmpdir(), "quotahorizon-timezone-"));
  try {
    const { prefix, destination, site } = timezoneFixture(work);
    const selected = bundleTimezoneData(prefix, destination, site);
    assert.deepEqual(selected.sort(), [join("share", "zoneinfo", "America", "Los_Angeles"), join("share", "zoneinfo", "UTC")]);
    assert.equal(readFileSync(join(destination, "share/zoneinfo/UTC"), "utf8"), "UTC fixture");
    const revision = () => createHash("sha256").update(JSON.stringify({ timezone: timezoneDigest(destination, site) })).digest("hex");
    const initial = revision();
    writeFileSync(join(destination, "share/zoneinfo/UTC"), "updated timezone data");
    const changedData = revision();
    assert.notEqual(changedData, initial);
    assert.equal(readFileSync(join(prefix, "share/zoneinfo/UTC"), "utf8"), "UTC fixture");
    writeFileSync(join(dirname(site), "sitecustomize.py"), readFileSync(join(dirname(site), "sitecustomize.py"), "utf8") + "# startup revision\n");
    assert.notEqual(revision(), changedData);
  } finally { rmSync(work, { recursive: true, force: true }); }
});

test("missing Conda timezone directory or required zone fails clearly", () => {
  const work = mkdtempSync(join(tmpdir(), "quotahorizon-timezone-missing-"));
  try {
    const { prefix, destination, site } = timezoneFixture(work);
    rmSync(join(prefix, "share/zoneinfo/America/Los_Angeles"));
    assert.throws(() => bundleTimezoneData(prefix, destination, site), /Missing Conda tzdata: share\/zoneinfo\/America\/Los_Angeles/);
    rmSync(join(prefix, "share/zoneinfo"), { recursive: true });
    assert.throws(() => bundleTimezoneData(prefix, destination, site), /Missing Conda tzdata/);
  } finally { rmSync(work, { recursive: true, force: true }); }
});

test("selected timezone files require and carry their actual Conda redistribution notices", () => {
  const work = mkdtempSync(join(tmpdir(), "quotahorizon-timezone-license-"));
  try {
    const { prefix, destination, site } = timezoneFixture(work);
    const used = new Set(bundleTimezoneData(prefix, destination, site));
    const cache = join(work, "package cache");
    mkdirSync(join(prefix, "conda-meta"));
    const meta = { name: "tzdata", version: "2025b", license: "LicenseRef-Public-Domain", files: [...used], link: { source: cache } };
    writeFileSync(join(prefix, "conda-meta/tzdata.json"), JSON.stringify(meta));
    assert.throws(() => copyRuntimeNotices(prefix, destination, used), /Missing redistribution notices for tzdata/);
    rmSync(join(destination, "licenses"), { recursive: true });
    mkdirSync(join(cache, "info/licenses"), { recursive: true });
    assert.throws(() => copyRuntimeNotices(prefix, destination, used), /Missing redistribution notices for tzdata/);
    rmSync(join(destination, "licenses"), { recursive: true });
    writeFileSync(join(cache, "info/licenses/LICENSE"), "Upstream notice fixture\n");
    assert.deepEqual(copyRuntimeNotices(prefix, destination, used), [{ name: meta.name, version: meta.version, license: meta.license }]);
    assert.equal(readFileSync(join(destination, "licenses/tzdata/LICENSE"), "utf8"), "Upstream notice fixture\n");
    rmSync(join(destination, "licenses"), { recursive: true });
    rmSync(join(prefix, "conda-meta/tzdata.json"));
    assert.throws(() => copyRuntimeNotices(prefix, destination, used), /Missing Conda tzdata package ownership/);
  } finally { rmSync(work, { recursive: true, force: true }); }
});

test("isolated Python uses relocated bundled zones for LA offsets, DST and dates; external zones cannot hide missing data", {
  skip: process.platform !== "darwin" || !process.env.QUOTAHORIZON_PYTHON ? "Requires an explicitly selected existing macOS Conda Python; installs nothing" : false,
}, () => {
  const work = mkdtempSync(join(tmpdir(), "quotahorizon-timezone-python-"));
  try {
    const python = process.env.QUOTAHORIZON_PYTHON;
    const info = JSON.parse(execFileSync(python, ["-I", "-B", "-c", "import sys,json; print(json.dumps({'prefix':sys.prefix,'version':f'{sys.version_info.major}.{sys.version_info.minor}','executable':sys.executable}))"], { encoding: "utf8" }));
    assert.ok(info.executable);
    const destination = join(work, "runtime before move");
    // Copy a disposable interpreter/stdlib fixture. A venv would inherit the
    // build host's stdlib sitecustomize instead of exercising bundle startup.
    mkdirSync(join(destination, "bin"), { recursive: true });
    cpSync(info.executable, join(destination, "bin/python3"), { dereference: true });
    cpSync(join(info.prefix, `lib/python${info.version}`), join(destination, `lib/python${info.version}`), {
      recursive: true, dereference: true,
      filter: p => !["site-packages", "__pycache__", "test", "tests", "idlelib", "tkinter", "ensurepip"].includes(basename(p)) && !p.endsWith(".pyc"),
    });
    const siteRelative = `lib/python${info.version}/site-packages`;
    mkdirSync(join(destination, siteRelative));
    writeFileSync(join(destination, `lib/python${info.version}/sitecustomize.py`), "raise RuntimeError('build host startup must be replaced')\n");
    bundleTimezoneData(info.prefix, destination, join(destination, siteRelative));
    const digest = timezoneDigest(destination, join(destination, siteRelative));
    const relocated = join(work, "relocated runtime with spaces");
    renameSync(destination, relocated);
    assert.equal(timezoneDigest(relocated, join(relocated, siteRelative)), digest);
    const bundledPython = join(relocated, "bin/python3");
    verifyRuntimeTimezone(bundledPython, relocated);
    // The base Conda TZPATH still contains working LA data. It must not be used
    // when the relocated runtime's copy is missing.
    execFileSync(python, ["-I", "-B", "-c", "from zoneinfo import ZoneInfo; ZoneInfo('America/Los_Angeles')"]);
    rmSync(join(relocated, "share/zoneinfo/America/Los_Angeles"));
    assert.throws(() => verifyRuntimeTimezone(bundledPython, relocated), /Missing bundled tzdata/);
    assert.throws(() => execFileSync(bundledPython, ["-I", "-B", "-c", "from zoneinfo import ZoneInfo; ZoneInfo('America/Los_Angeles')"], {
      cwd: "/", env: { PATH: "/usr/bin:/bin", PYTHONTZPATH: join(info.prefix, "share/zoneinfo") }, stdio: "pipe",
    }), /ZoneInfoNotFoundError/);
    rmSync(join(relocated, "share/zoneinfo"), { recursive: true });
    assert.throws(() => verifyRuntimeTimezone(bundledPython, relocated), /Missing bundled tzdata/);
  } finally { rmSync(work, { recursive: true, force: true }); }
});

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
