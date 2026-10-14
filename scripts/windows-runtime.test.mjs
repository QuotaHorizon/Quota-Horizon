import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import { verifyDownload } from "./prepare-windows-runtime.mjs";

test("runtime preparation rejects a modified archive before extraction", () => {
  const data = Buffer.from("pinned runtime archive");
  const expected = createHash("sha256").update(data).digest("hex");
  verifyDownload(data, expected, "fixture");
  assert.throws(() => verifyDownload(Buffer.from("modified archive"), expected, "fixture"), /checksum differs/);
});
