fn main() {
    // Only the standalone smoke host owns executable resources. Emitting them
    // from this service library duplicates the desktop host's Windows VERSION.
    if std::env::var_os("CARGO_FEATURE_NATIVE_SMOKE").is_some() {
        tauri_build::build();
    }
}
