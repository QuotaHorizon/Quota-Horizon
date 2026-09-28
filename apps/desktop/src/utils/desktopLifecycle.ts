// A browser preview/hosted page must not inherit a native macOS-only policy.
export function isMenuBarResidentDesktop(
  desktop = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window,
  platform = typeof navigator === "undefined" ? "" : navigator.platform,
): boolean {
  return desktop && platform.toLowerCase().includes("mac");
}
