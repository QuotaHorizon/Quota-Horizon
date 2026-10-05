import { mkdirSync, readFileSync, renameSync, writeFileSync, existsSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

export function connectionOptions(args) {
  const values = new Map();
  const flags = ["--forecast", "--evaluation", "--data-dir", "--python", "--observer-dir", "--observer-db", "--producer-db", "--config", "--html"];
  let external = false;
  for (let i = 0; i < args.length; i++) {
    const key = args[i];
    if (key === "--external") { external = true; continue; }
    if (!flags.includes(key) || values.has(key) || !args[i+1] || args[i+1].startsWith("--")) throw new Error(`Invalid option: ${key}`);
    values.set(key, resolve(args[++i]));
  }
  if (!values.has("--forecast") || !values.has("--evaluation")) throw new Error("Provide --forecast and --evaluation JSON paths.");
  const managed = flags.slice(3).some(flag => values.has(flag));
  if (managed && external) throw new Error("Choose managed startup or --external.");
  if (managed && ["--python", "--observer-dir", "--observer-db"].some(flag => !values.has(flag))) throw new Error("Managed startup requires --python, --observer-dir and --observer-db.");
  return { values, managed, external };
}

function atomicWrite(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, JSON.stringify(value, null, 2) + "\n", { mode: 0o600 });
  renameSync(temporary, path);
}
export function connect(args) {
  const { values, managed, external } = connectionOptions(args);
  const paths = { forecastPath: values.get("--forecast"), evaluationPath: values.get("--evaluation") };
  for (const [path, schema] of [[paths.forecastPath, "reset-intelligence-consumer/1"], [paths.evaluationPath, "reset-observer/1"]]) {
    if (JSON.parse(readFileSync(path, "utf8")).schema_version !== schema) throw new Error(`Expected ${schema}`);
  }
  const root = process.platform === "darwin" ? join(homedir(), "Library", "Application Support")
    : process.platform === "win32" ? process.env.APPDATA : process.env.XDG_DATA_HOME || join(homedir(), ".local", "share");
  const directory = values.get("--data-dir") || (root && join(root, "io.github.halfmelon.quotahorizon"));
  if (!directory || !isAbsolute(directory)) throw new Error("An absolute --data-dir is required on this platform.");
  const connectionPath = join(directory, "reset-radar-connection.json"), runtimePath = join(directory, "reset-radar-runtime.json");
  if (!managed && !external && existsSync(runtimePath)) {
    const current = JSON.parse(readFileSync(connectionPath, "utf8"));
    if (current.forecastPath !== paths.forecastPath || current.evaluationPath !== paths.evaluationPath) throw new Error("Reconnect managed modules with their runtime options, or select --external.");
  }
  let runtime;
  if (managed) {
    runtime = { pythonPath: values.get("--python"), moduleDirectory: values.get("--observer-dir"),
      databasePath: values.get("--observer-db"), producerDatabasePath: values.get("--producer-db") ?? null,
      configPath: values.get("--config") ?? null, outputPath: paths.evaluationPath, htmlPath: values.get("--html") ?? null };
    if (!existsSync(join(runtime.moduleDirectory, "reset_observer/__main__.py"))) throw new Error("Observer module directory is unavailable.");
    for (const p of [runtime.databasePath, runtime.producerDatabasePath, runtime.configPath].filter(Boolean)) {
      if (!existsSync(p)) throw new Error("Initialize the module database and configuration before connecting managed startup.");
    }
    // Verify the selected existing environment; never installs dependencies.
    const checked = spawnSync(runtime.pythonPath, ["-B", "-c", "import sys,numpy,reset_observer.__main__; assert sys.version_info >= (3,11); assert 2 <= int(numpy.__version__.split('.')[0]) < 3"],
      { cwd: runtime.moduleDirectory, stdio: "ignore", timeout: 10000, windowsHide: true });
    if (checked.error || checked.status !== 0) throw new Error("The selected Python must already provide Python 3.11+ and NumPy 2.x.");
  }
  atomicWrite(connectionPath, paths);
  if (runtime) atomicWrite(runtimePath, runtime);
  if (external) rmSync(runtimePath, { force: true });
  console.log(runtime ? "Connected. Horizon will keep the independent observer running, including its configured producer."
    : "Connected to the independent JSON exports. Existing module processes keep their own lifecycle.");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { connect(process.argv.slice(2)); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
