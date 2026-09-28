import { test } from "node:test";
import assert from "node:assert/strict";
import { desktopSigningPlan } from "./desktop-signing.mjs";

const plan = (extra = {}) => desktopSigningPlan({ platform: "darwin", args: ["--bundles", "app"],
  config: { bundle: { macOS: { signingIdentity: "-" } } }, environment: {}, ...extra });

test("local ad-hoc builds explicitly retain the upgrade authorization limitation", () => {
  assert.deepEqual(plan(), { args: ["--bundles", "app", "--no-sign"], sealAdHoc: true, mode: "ad_hoc" });
});
test("a configured Apple identity is not discarded when the updater key is absent", () => {
  const actual = plan({ environment: { APPLE_SIGNING_IDENTITY: "Synthetic Test Identity" } });
  assert.equal(actual.mode, "certificate");
  assert.equal(actual.sealAdHoc, false);
  assert.ok(!actual.args.includes("--no-sign"));
  assert.equal(plan({ config: { bundle: { macOS: { signingIdentity: "Synthetic Test Identity" } } } }).mode, "certificate");
});
test("ambiguous certificate plus no-sign stops before building, without printing identity values", () => {
  assert.throws(() => plan({ args: ["--no-sign"], environment: { APPLE_SIGNING_IDENTITY: "Synthetic Test Identity" } }), /would bypass it/);
});
test("updater signing does not imply a stable application identity", () => {
  const actual = plan({ environment: { TAURI_SIGNING_PRIVATE_KEY: "synthetic-not-a-key" } });
  assert.equal(actual.mode, "ad_hoc");
  assert.equal(actual.sealAdHoc, false);
});
test("non-macOS builds never receive a macOS seal", () => {
  assert.equal(plan({ platform: "win32" }).mode, "not_applicable");
  assert.equal(plan({ platform: "win32" }).sealAdHoc, false);
});
