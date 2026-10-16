import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, cpSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const run = (command, args) => execFileSync(command, args, { encoding: "utf8", maxBuffer: 8 * 1024 * 1024 });
const hash = data => createHash("sha256").update(data).digest("hex");
export function moduleDigest() {
  return hash(["reset-intelligence/reset_intelligence", "reset-observer/reset_observer"].flatMap(p =>
    readdirSync(join(root, "modules", p)).filter(n => n.endsWith(".py")).sort().map(n =>
      `${p}/${n}:${hash(readFileSync(join(root, "modules", p, n)))}`)).join("\n"));
}
function files(directory) {
  return readdirSync(directory).flatMap(n => {
    const p = join(directory, n);
    return statSync(p).isDirectory() ? files(p) : [p];
  });
}
const systemLibrary = p => p.startsWith("/usr/lib/") || p.startsWith("/System/Library/");
const libraries = p => run("/usr/bin/otool", ["-L", p]).trim().split("\n").slice(1).map(s => s.trim().split(" (compatibility version")[0]);
const macho = p => [0xfeedfacf, 0xcffaedfe, 0xcafebabe, 0xbebafeca].includes(readFileSync(p).subarray(0, 4).readUInt32BE());

export function normalizeRuntimePermissions(directory) {
  const stat = lstatSync(directory);
  if (!stat.isDirectory()) throw new Error("Runtime permissions require a real directory");
  chmodSync(directory, 0o755);
  for (const name of readdirSync(directory)) {
    const path = join(directory, name);
    const entry = lstatSync(path);
    if (entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) normalizeRuntimePermissions(path);
    else if (entry.isFile()) chmodSync(path, entry.mode & 0o111 ? 0o755 : 0o644);
    else throw new Error(`Unsupported runtime entry: ${path}`);
  }
}

export function bundleTimezoneData(prefix, destination, site) {
  const source = join(prefix, "share/zoneinfo");
  for (const name of ["UTC", "America/Los_Angeles"]) {
    if (!existsSync(join(source, name)) || !statSync(join(source, name)).isFile()) throw new Error(`Missing Conda tzdata: share/zoneinfo/${name}`);
  }
  cpSync(source, join(destination, "share/zoneinfo"), { recursive: true, dereference: true });
  // -I still loads sitecustomize. Resolve from the installed prefix, never the
  // build host's compiled TZPATH or an environment variable.
  // The stdlib precedes site-packages and Conda can supply its own startup
  // hook there. Replace that copied hook rather than let it shadow ours.
  writeFileSync(join(dirname(site), "sitecustomize.py"), "import os\nimport sys\nimport zoneinfo\n\nzoneinfo.reset_tzpath((os.path.join(sys.prefix, 'share', 'zoneinfo'),))\n");
  return files(source).map(p => relative(prefix, p));
}

export function timezoneDigest(destination, site) {
  return hash([...files(join(destination, "share/zoneinfo")), join(dirname(site), "sitecustomize.py")].sort().map(p =>
    `${relative(destination, p)}:${hash(readFileSync(p))}`).join("\n"));
}

export function copyRuntimeNotices(prefix, destination, used) {
  const notices = join(destination, "licenses"); mkdirSync(notices);
  const packages = [];
  for (const entry of readdirSync(join(prefix, "conda-meta")).filter(n => n.endsWith(".json"))) {
    const meta = JSON.parse(readFileSync(join(prefix, "conda-meta", entry), "utf8"));
    if (!meta.files?.some(p => used.has(p))) continue;
    const source = meta.link?.source && join(meta.link.source, "info/licenses");
    if (!source || !existsSync(source) || !files(source).length) throw new Error(`Missing redistribution notices for ${meta.name}`);
    cpSync(source, join(notices, meta.name), { recursive: true });
    packages.push({ name: meta.name, version: meta.version, license: meta.license });
  }
  if (!packages.some(p => p.name === "tzdata")) throw new Error("Missing Conda tzdata package ownership and redistribution notices");
  return packages;
}

export function verifyRuntimeTimezone(python, destination) {
  execFileSync(python, ["-I", "-B", "-c", `
import os, sys, zoneinfo
from datetime import datetime, timedelta, timezone
path = os.path.join(sys.prefix, 'share', 'zoneinfo')
assert os.path.realpath(sys.prefix) == os.path.realpath(sys.argv[1]), 'Timezone prefix escapes bundle'
assert zoneinfo.TZPATH == (path,), 'Timezone search path escapes bundle'
for name in ('UTC', 'America/Los_Angeles'):
    assert os.path.isfile(os.path.join(path, name)), 'Missing bundled tzdata: ' + name
la = zoneinfo.ZoneInfo('America/Los_Angeles')
assert datetime(2026, 1, 15, tzinfo=la).utcoffset() == timedelta(hours=-8)
assert datetime(2026, 7, 15, tzinfo=la).utcoffset() == timedelta(hours=-7)
for utc, expected in (
    ('2026-03-08T09:59:59', '2026-03-08T01:59:59-08:00'),
    ('2026-03-08T10:00:00', '2026-03-08T03:00:00-07:00'),
    ('2026-11-01T08:59:59', '2026-11-01T01:59:59-07:00'),
    ('2026-11-01T09:00:00', '2026-11-01T01:00:00-08:00'),
    ('2026-03-08T07:59:59', '2026-03-07T23:59:59-08:00'),
    ('2026-03-08T08:00:00', '2026-03-08T00:00:00-08:00'),
    ('2026-11-01T06:59:59', '2026-10-31T23:59:59-07:00'),
    ('2026-11-01T07:00:00', '2026-11-01T00:00:00-07:00'),
):
    assert datetime.fromisoformat(utc).replace(tzinfo=timezone.utc).astimezone(la).isoformat() == expected
`, destination], { cwd: "/", encoding: "utf8", env: { PATH: "/usr/bin:/bin" } });
}

export function verifyRuntime(destination) {
  const manifest = JSON.parse(readFileSync(join(destination, "reset-runtime-manifest.json"), "utf8"));
  for (const p of files(destination).filter(p => statSync(p).size >= 4 && macho(p))) {
    if (run("/usr/bin/lipo", ["-archs", p]).trim() !== "arm64") throw new Error(`Unexpected runtime architecture: ${relative(destination, p)}`);
    const load = run("/usr/bin/otool", ["-l", p]);
    const minimum = load.match(/\bminos (\d+(?:\.\d+)*)/)?.[1] ?? load.match(/cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version (\d+(?:\.\d+)*)/)?.[1];
    const parts = minimum?.split(".").map(Number) ?? [];
    if (!minimum || parts[0] > 13 || (parts[0] === 13 && parts.slice(1).some(n => n > 0))) throw new Error(`Runtime requires macOS ${minimum ?? "unknown"}: ${relative(destination, p)}`);
    for (const dep of libraries(p)) {
      if (systemLibrary(dep)) continue;
      if (!dep.startsWith("@loader_path/") || !existsSync(resolve(dirname(p), dep.slice(13)))) throw new Error(`External runtime dependency: ${dep}`);
      if (!resolve(dirname(p), dep.slice(13)).startsWith(destination + "/")) throw new Error("Runtime dependency escapes bundle");
    }
  }
  const python = join(destination, "bin", "python3");
  const check = JSON.parse(execFileSync(python, ["-I", "-B", "-c", "import sys,ssl,sqlite3,numpy,reset_intelligence.__main__,reset_observer.__main__,json; print(json.dumps({'prefix':sys.prefix,'numpy':numpy.__version__,'paths':sys.path,'inverse':numpy.linalg.inv(numpy.eye(2)).tolist(),'certs':ssl.create_default_context().cert_store_stats()['x509_ca']}))"], {
    cwd: "/", encoding: "utf8", env: { PATH: "/usr/bin:/bin", SSL_CERT_FILE: join(destination, "certs.pem") },
  }));
  if (realpathSync(check.prefix) !== realpathSync(destination) || check.paths.some(p => p && !p.startsWith(destination + "/")) || !check.certs) throw new Error("Runtime isolation or certificate verification failed");
  verifyRuntimeTimezone(python, destination);
  return manifest;
}

export function bundleRuntime(python, destination = join(root, "dist/reset-runtime")) {
  if (process.platform !== "darwin" || process.arch !== "arm64") throw new Error("The bundled runtime currently requires an Apple Silicon macOS build host.");
  if (!python) throw new Error("Set QUOTAHORIZON_PYTHON to an existing Conda Python 3.11+ environment with NumPy 2.x and macOS 13-compatible binaries. No dependencies are installed by this build.");
  const info = JSON.parse(run(python, ["-I", "-c", "import sys,numpy,pathlib,json; assert (3,11)<=sys.version_info[:2]<(3,15); assert numpy.__version__.startswith('2.'); print(json.dumps({'prefix':sys.prefix,'python':sys.executable,'version':f'{sys.version_info.major}.{sys.version_info.minor}','numpy':numpy.__version__,'numpy_path':str(pathlib.Path(numpy.__file__).parent)}))"]));
  const prefix = realpathSync(info.prefix);
  // Only selected distributable runtime files enter the artifact. No environment
  // archive, source configuration, databases or user site-packages are copied.
  rmSync(destination, { recursive: true, force: true });
  mkdirSync(join(destination, "bin"), { recursive: true });
  cpSync(realpathSync(info.python), join(destination, "bin/python3"));
  const libraryPath = `lib/python${info.version}`;
  const lib = join(destination, libraryPath);
  const excluded = new Set(["site-packages", "__pycache__", "test", "tests", "idlelib", "tkinter", "turtledemo", "ensurepip"]);
  cpSync(join(prefix, libraryPath), lib, { recursive: true, dereference: true,
    filter: p => !excluded.has(basename(p)) && !/^(?:config-|_test|xx|_tkinter|_curses|readline)/.test(basename(p)) && !/\.(?:pyc|a)$/.test(p) });
  const site = join(lib, "site-packages");
  mkdirSync(site);
  const timezoneFiles = bundleTimezoneData(prefix, destination, site);
  cpSync(info.numpy_path, join(site, "numpy"), { recursive: true, dereference: true, filter: p => !["tests", "__pycache__"].includes(basename(p)) && !p.endsWith(".pyc") });
  cpSync(join(dirname(info.numpy_path), `numpy-${info.numpy}.dist-info`), join(site, `numpy-${info.numpy}.dist-info`), { recursive: true });
  for (const [directory, name] of [["reset-intelligence", "reset_intelligence"], ["reset-observer", "reset_observer"]]) {
    cpSync(join(root, "modules", directory, name), join(site, name), { recursive: true, filter: p => !["__pycache__"].includes(basename(p)) && !p.endsWith(".pyc") });
  }
  cpSync(realpathSync(join(prefix, "ssl/cacert.pem")), join(destination, "certs.pem"));
  const queue = files(destination).filter(p => statSync(p).size >= 4 && macho(p));
  const originals = new Map(queue.map(p => [p, p === join(destination, "bin/python3") ? realpathSync(info.python) : join(prefix, relative(destination, p))]));
  for (let i = 0; i < queue.length; i++) {
    const p = queue[i], original = originals.get(p);
    for (const dep of libraries(p)) {
      if (systemLibrary(dep)) continue;
      const source = dep.startsWith("@rpath/") ? join(prefix, "lib", dep.slice(7))
        : dep.startsWith("@loader_path/") ? resolve(dirname(original), dep.slice(13)) : dep;
      if (!source.startsWith(prefix + "/") || !existsSync(source)) throw new Error(`Cannot bundle dependency: ${dep}`);
      const dest = join(destination, "lib", basename(source));
      if (!existsSync(dest)) { cpSync(realpathSync(source), dest); queue.push(dest); originals.set(dest, realpathSync(source)); }
      run("/usr/bin/install_name_tool", ["-change", dep, `@loader_path/${relative(dirname(p), dest)}`, p]);
    }
    if (p.endsWith(".dylib")) run("/usr/bin/install_name_tool", ["-id", `@loader_path/${basename(p)}`, p]);
    // Remove build-machine search paths after linking every non-system library
    // directly to another bundled file.
    const load = run("/usr/bin/otool", ["-l", p]);
    for (const m of load.matchAll(/cmd LC_RPATH\s+cmdsize \d+\s+path (.+?) \(offset/g)) run("/usr/bin/install_name_tool", ["-delete_rpath", m[1], p]);
    run("/usr/bin/codesign", ["--force", "--sign", "-", p]);
  }
  // Conda's package cache contains the upstream licenses for its runtime
  // binaries. Copy notices for the packages owning the selected files only.
  const used = new Set([...originals.values(), realpathSync(join(prefix, "ssl/cacert.pem"))].map(p => relative(prefix, p)).concat(timezoneFiles));
  const packages = copyRuntimeNotices(prefix, destination, used);
  cpSync(join(prefix, libraryPath, "LICENSE.txt"), join(destination, "licenses/PYTHON-LICENSE.txt"));
  const manifest = { schema_version: "reset-runtime/1", platform: "macos-arm64", python: info.version, numpy: info.numpy, modules: moduleDigest(), packages, timezone: timezoneDigest(destination, site) };
  manifest.revision = hash(JSON.stringify(manifest));
  writeFileSync(join(destination, "reset-runtime-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  // Conda caches can contain group-writable files. DMG copying applies umask;
  // give only the generated runtime stable distribution modes before bundling.
  normalizeRuntimePermissions(destination);
  verifyRuntime(destination);
  console.log(`Bundled independent reset modules and verified their isolated runtime (${manifest.revision.slice(0, 12)}).`);
  return destination;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { bundleRuntime(process.env.QUOTAHORIZON_PYTHON); } catch (e) { console.error(e.message); process.exitCode = 1; }
}
