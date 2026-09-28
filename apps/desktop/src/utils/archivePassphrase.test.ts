import { describe, expect, it } from "vitest";
import {
  ARCHIVE_PASSPHRASE_MIN_CHARS,
  ARCHIVE_PASSPHRASE_MAX_CHARS,
  validateArchivePassphrase,
} from "./archivePassphrase";

describe("validateArchivePassphrase", () => {
  it("enforces the backend minimum using Unicode characters", () => {
    expect(ARCHIVE_PASSPHRASE_MIN_CHARS).toBe(12);
    expect(validateArchivePassphrase("short")).toBe("tooShort");
    expect(validateArchivePassphrase("配额地平线安全备份口令甲乙")).toBeNull();
    expect(validateArchivePassphrase("x".repeat(ARCHIVE_PASSPHRASE_MAX_CHARS + 1)))
      .toBe("tooLong");
  });

  it("requires matching confirmation only when one is supplied", () => {
    expect(validateArchivePassphrase("correct horse battery staple")).toBeNull();
    expect(validateArchivePassphrase("correct horse battery staple", "different phrase"))
      .toBe("mismatch");
    expect(validateArchivePassphrase(
      "correct horse battery staple",
      "correct horse battery staple",
    )).toBeNull();
  });
});
