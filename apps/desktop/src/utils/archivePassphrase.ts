export const ARCHIVE_PASSPHRASE_MIN_CHARS = 12;
export const ARCHIVE_PASSPHRASE_MAX_CHARS = 1024;

export type ArchivePassphraseIssue = "tooShort" | "tooLong" | "mismatch";

export function validateArchivePassphrase(
  passphrase: string,
  confirmation?: string,
): ArchivePassphraseIssue | null {
  if ([...passphrase].length < ARCHIVE_PASSPHRASE_MIN_CHARS) return "tooShort";
  if ([...passphrase].length > ARCHIVE_PASSPHRASE_MAX_CHARS) return "tooLong";
  if (confirmation !== undefined && passphrase !== confirmation) return "mismatch";
  return null;
}
