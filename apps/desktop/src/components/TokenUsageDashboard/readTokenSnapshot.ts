/** Wait for both reads even if one fails, so the next refresh cannot overlap it. */
export async function readTokenSnapshot<Entries, Daily>(
  readEntries: () => Promise<Entries>, readDaily: () => Promise<Daily>,
): Promise<[Entries, Daily]> {
  const [entries, daily] = await Promise.allSettled([
    Promise.resolve().then(readEntries), Promise.resolve().then(readDaily),
  ]);
  if (entries.status === "rejected") throw entries.reason;
  if (daily.status === "rejected") throw daily.reason;
  return [entries.value, daily.value];
}
