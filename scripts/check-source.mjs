import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Dependency-free checks for a source checkout, including before its first commit.
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const files = [...new Set(execFileSync("git", [
  "ls-files", "--cached", "--others", "--exclude-standard", "-z",
], { cwd: root, encoding: "utf8" }).split("\0").filter(Boolean))];
const candidates = new Set(files);
const errors = [];
const read = (file) => readFileSync(join(root, file), "utf8");
const json = (file) => JSON.parse(read(file));

for (const file of [
  "LICENSE", "NOTICE", "UPSTREAM.md", "Cargo.lock",
  "apps/desktop/package-lock.json", "apps/desktop/src-tauri/Cargo.lock",
  "apps/capacity-preview/src-tauri/Cargo.toml", "apps/capacity-preview/src/status.ts",
]) {
  if (!candidates.has(file)) errors.push(`Required source file is missing or ignored: ${file}`);
}

const desktop = json("apps/desktop/package.json");
const locked = json("apps/desktop/package-lock.json").packages[""];
for (const field of ["name", "version"]) {
  if (desktop[field] !== locked[field]) errors.push(`Desktop lockfile differs: ${field}`);
}
for (const field of ["dependencies", "devDependencies"]) {
  const declared = desktop[field] ?? {};
  const recorded = locked[field] ?? {};
  for (const name of new Set([...Object.keys(declared), ...Object.keys(recorded)])) {
    if (declared[name] !== recorded[name]) errors.push(`Desktop lockfile differs: ${field}.${name}`);
  }
}

let pathDependencies = 0;
let documentLinks = 0;
for (const file of files) {
  if (!existsSync(join(root, file))) {
    errors.push(`Listed source file is absent: ${file}`);
    continue;
  }
  if (/(^|\/)(node_modules|target|dist|private-data|\.local)(\/|$)/.test(file)) {
    errors.push(`Generated or private content is included: ${file}`);
  }
  if (/(^|\/)Cargo\.toml$/.test(file)) {
    for (const match of read(file).matchAll(/\bpath\s*=\s*"([^"]+)"/g)) {
      // Inline dependency tables use path directories; lib/bin entries name files.
      if (/\.rs$/.test(match[1])) continue;
      const target = resolve(root, dirname(file), match[1], "Cargo.toml");
      const name = relative(root, target);
      if (name.startsWith("..") || isAbsolute(name) || !candidates.has(name)) {
        errors.push(`Rust dependency is outside the source package or missing: ${file} -> ${match[1]}`);
      }
      pathDependencies++;
    }
  }
  if (file.endsWith(".md")) {
    const markdown = read(file).replace(/^```[^\n]*\n[\s\S]*?^```/gm, "");
    for (const match of markdown.matchAll(/!?\[[^\]]*\]\((<[^>]+>|[^\s)]+)(?:\s+"[^"]*")?\)/g)) {
      const link = match[1].replace(/^<|>$/g, "");
      if (/^(?:[a-z][a-z\d+.-]*:|#|\/\/)/i.test(link)) continue;
      const target = decodeURIComponent(link.split(/[?#]/)[0]);
      if (!target) continue;
      const name = relative(root, resolve(root, dirname(file), target));
      const included = candidates.has(name) || files.some((entry) => entry.startsWith(`${name}/`));
      if (!included) errors.push(`Document target is missing or private: ${file} -> ${link}`);
      documentLinks++;
    }
  }
}

if (errors.length) {
  errors.forEach((error) => console.error(error));
  process.exitCode = 1;
} else {
  console.log(`Source checks passed: ${files.length} files, ${pathDependencies} Rust path dependencies, ${documentLinks} document links; desktop lockfile matches.`);
}
