fn main() {
    println!("cargo:rerun-if-changed=src/platform/macos/native.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // The compiler takes all Darwin and IOKit layouts from the selected Apple SDK.
    let sdk = std::process::Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("xcrun is required for the macOS adapter");
    assert!(sdk.status.success(), "xcrun could not locate the macOS SDK");
    let sdk = String::from_utf8(sdk.stdout).expect("SDK path is UTF-8");
    cc::Build::new()
        .file("src/platform/macos/native.c")
        .flag("-isysroot")
        .flag(sdk.trim())
        .flag("-std=c11")
        .warnings(true)
        .compile("storage_native");
    println!("cargo:rustc-link-lib=framework=IOKit");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}
