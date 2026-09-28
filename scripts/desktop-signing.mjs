// Updater signatures and macOS code signatures are independent. A missing
// updater key must never silently downgrade a configured certificate to ad hoc.
export function desktopSigningPlan({ platform, args, config, environment }) {
  const identity = environment.APPLE_SIGNING_IDENTITY?.trim()
    || config.bundle?.macOS?.signingIdentity?.trim();
  const certificate = platform === "darwin" && identity && identity !== "-";
  if (certificate && args.includes("--no-sign")) {
    throw new Error("A macOS signing identity is configured, but --no-sign would bypass it. Remove --no-sign or explicitly select an ad-hoc local build.");
  }
  const nextArgs = [...args];
  if (!certificate && !environment.TAURI_SIGNING_PRIVATE_KEY?.trim() && !nextArgs.includes("--no-sign")) nextArgs.push("--no-sign");
  return {
    args: nextArgs,
    sealAdHoc: platform === "darwin" && nextArgs.includes("--no-sign"),
    mode: platform !== "darwin" ? "not_applicable" : certificate ? "certificate" : "ad_hoc",
  };
}
