fn main() {
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_macos_lifecycle();
    }
}

fn build_macos_lifecycle() {
    use std::{path::PathBuf, process::Command};
    println!("cargo:rerun-if-changed=src/macos_termination.m");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("unsupported macOS architecture: {other:?}"),
    };
    // Use the same installed Apple toolchain as Tauri, with no new build crate
    // or global dependency. Cargo supplies a separate OUT_DIR for each target.
    let object = out.join("macos_termination.o");
    assert!(Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "clang",
            "-arch",
            arch,
            "-mmacosx-version-min=13.0",
            "-fobjc-arc",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-c",
            "src/macos_termination.m",
            "-o"
        ])
        .arg(&object)
        .status()
        .expect("Apple clang is required")
        .success());
    assert!(Command::new("xcrun")
        .args(["ar", "crs"])
        .arg(out.join("libhorizon_lifecycle.a"))
        .arg(object)
        .status()
        .expect("Apple ar is required")
        .success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=horizon_lifecycle");
    println!("cargo:rustc-link-lib=framework=AppKit");
}
