#!/bin/sh
# Product-base checks for an independent checkout, including before its first commit.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
product_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
desktop_root="$product_root/apps/desktop"

fail() {
  printf 'product-base verification failed: %s\n' "$1" >&2
  exit 1
}

for required in \
  "$product_root/LICENSE" \
  "$product_root/NOTICE" \
  "$product_root/UPSTREAM.md" \
  "$desktop_root/package.json" \
  "$desktop_root/package-lock.json" \
  "$desktop_root/src-tauri/icons/128x128.png" \
  "$desktop_root/src-tauri/Cargo.toml"; do
  [ -f "$required" ] || fail "required provenance or manifest file is missing"
done

unexpected=$(git -C "$product_root" ls-files apps/desktop | rg \
  '(^|/)(\.git|\.github|node_modules|target|dist)(/|$)|(^|/)project\.json$|/resources/dream-skin/' \
  | head -n 1 || true)
[ -z "$unexpected" ] || fail "an excluded upstream path is tracked"

license_blob=$(git -C "$product_root" hash-object LICENSE)
[ "$license_blob" = "42c07bfac2f49da31c056c5619c3a005da29e6e8" ] \
  || fail "the fixed Apache-2.0 license text changed"

if rg -q 'piperhex/codex-switch|tauri-plugin-updater|plugin-updater' \
  "$desktop_root/package.json" \
  "$desktop_root/src" \
  "$desktop_root/src-tauri/Cargo.toml" \
  "$desktop_root/src-tauri/src" \
  "$desktop_root/src-tauri/tauri.conf.json"; then
  fail "the runnable shell still contains the excluded upstream update path"
fi

[ ! -e "$desktop_root/src/pages/DreamSkinPage.tsx" ] \
  || fail "the excluded Dream Skin page still exists"
[ ! -e "$desktop_root/src-tauri/src/dream_skin.rs" ] \
  || fail "the excluded Dream Skin backend still exists"
rg -q '"active": true' "$desktop_root/src-tauri/tauri.conf.json" \
  || fail "macOS app bundling is not enabled"

rg -q 'apps/capacity-preview/src-tauri' "$product_root/Cargo.toml" \
  || fail "the existing read-plane desktop is not in the active Cargo workspace"
rg -q 'exclude = \["apps/desktop/src-tauri"\]' "$product_root/Cargo.toml" \
  || fail "the desktop's separate Cargo workspace declaration changed"
[ -f "$product_root/apps/capacity-preview/src/status.ts" ] \
  || fail "the desktop shared IPC types are missing"

if rg -l --hidden -g '!node_modules/**' -g '!target/**' -g '!dist/**' \
  'BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|AKIA[0-9A-Z]{16}|sk-[A-Za-z0-9]{24,}' \
  "$desktop_root" >/dev/null; then
  fail "a credential-shaped string exists in the imported desktop tree"
fi

file_count=$(git -C "$product_root" ls-files --cached --others --exclude-standard apps/desktop | sort -u | wc -l | tr -d ' ')
[ "$file_count" -ge 350 ] || fail "the imported desktop tree is unexpectedly incomplete"

printf 'product-base verification passed (%s files)\n' "$file_count"
