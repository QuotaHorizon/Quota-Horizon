import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const windowsRuntimeBase = join(root, "dist/windows-runtime-base");
export const windowsRuntimeLock = JSON.parse(readFileSync(join(root, ".github/ci/windows-runtime.lock.json"), "utf8"));
const hash = data => createHash("sha256").update(data).digest("hex");

export function verifyDownload(data, expected, name) {
  if (hash(data) !== expected) throw new Error(`Runtime download checksum differs: ${name}`);
}

export async function prepareWindowsRuntime() {
  if (process.platform !== "win32" || process.arch !== "x64") throw new Error("Prepare the Windows x64 runtime on a Windows x64 host.");
  const downloads = join(root, "dist/runtime-downloads");
  mkdirSync(downloads, { recursive: true });
  const archives = {};
  for (const [name, entry] of Object.entries(windowsRuntimeLock)) {
    const path = join(downloads, basename(new URL(entry.url).pathname));
    if (!existsSync(path)) {
      console.log(`Downloading ${name} ${entry.version} into the project...`);
      const response = await fetch(entry.url, { signal: AbortSignal.timeout(120_000) });
      if (!response.ok) throw new Error(`Runtime download failed: HTTP ${response.status}`);
      const data = Buffer.from(await response.arrayBuffer());
      verifyDownload(data, entry.sha256, name);
      writeFileSync(path, data);
    }
    verifyDownload(readFileSync(path), entry.sha256, name);
    archives[name] = path;
  }
  // This fixed generated directory never contains a developer environment or user data.
  rmSync(windowsRuntimeBase, { recursive: true, force: true });
  mkdirSync(windowsRuntimeBase, { recursive: true });
  execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command",
    "$ErrorActionPreference = 'Stop'; Expand-Archive -LiteralPath $env:QUOTAHORIZON_ARCHIVE -DestinationPath $env:QUOTAHORIZON_RUNTIME"], {
    windowsHide: true,
    env: { ...process.env, QUOTAHORIZON_ARCHIVE: archives.python, QUOTAHORIZON_RUNTIME: windowsRuntimeBase },
  });
  const python = join(windowsRuntimeBase, "python.exe");
  const site = join(windowsRuntimeBase, "Lib/site-packages");
  mkdirSync(site, { recursive: true });
  for (const [name, archive] of Object.entries(archives)) {
    if (name === "python") continue;
    execFileSync(python, ["-I", "-B", "-c",
      "import sys,zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])", archive, site], { windowsHide: true });
  }
  const pathFile = readdirSync(windowsRuntimeBase).find(n => /^python\d+\._pth$/.test(n));
  if (!pathFile) throw new Error("Python's isolated path configuration is missing");
  writeFileSync(join(windowsRuntimeBase, pathFile), `${pathFile.replace("._pth", ".zip")}\n.\nLib/site-packages\n`);
  const licenses = join(windowsRuntimeBase, "licenses");
  mkdirSync(licenses);
  cpSync(join(windowsRuntimeBase, "LICENSE.txt"), join(licenses, "PYTHON-LICENSE.txt"));
  cpSync(join(site, `numpy-${windowsRuntimeLock.numpy.version}.dist-info/LICENSE.txt`), join(licenses, "NUMPY-LICENSE.txt"));
  cpSync(join(site, `tzdata-${windowsRuntimeLock.tzdata.version}.dist-info/licenses`), join(licenses, "tzdata"), { recursive: true });
  writeFileSync(join(windowsRuntimeBase, "windows-runtime-lock.json"), JSON.stringify(windowsRuntimeLock, null, 2) + "\n");
  console.log(`Prepared project runtime: ${python}`);
  return windowsRuntimeBase;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { await prepareWindowsRuntime(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
