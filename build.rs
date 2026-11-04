//! ONNX Runtime's macOS static library references `___isPlatformVersionAtLeast`,
//! which lives in compiler-rt. Rust's linker does not add it, so Parakeet builds
//! link it explicitly. Only active on macOS with `--features parakeet`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::consts::OS != "macos" || std::env::var_os("CARGO_FEATURE_PARAKEET").is_none() {
        return;
    }
    let Ok(output) = std::process::Command::new("clang")
        .arg("-print-resource-dir")
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let resource_dir = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    println!("cargo:rustc-link-search=native={resource_dir}/lib/darwin");
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}
