//! Native build steps.
//!
//! - `native/system_audio_tap.m` wraps the Core Audio process tap API
//!   (macOS 14.2+), so System Audio capture does not require BlackHole.
//! - ONNX Runtime's macOS static library references `___isPlatformVersionAtLeast`,
//!   which lives in compiler-rt. Rust's linker does not add it, so Parakeet
//!   builds link it explicitly.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::consts::OS != "macos" {
        return;
    }
    build_system_audio_tap();
    if std::env::var_os("CARGO_FEATURE_PARAKEET").is_some() {
        link_compiler_rt();
    }
}

fn build_system_audio_tap() {
    println!("cargo:rerun-if-changed=native/system_audio_tap.m");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let object = out_dir.join("system_audio_tap.o");
    let archive = out_dir.join("libscrib_system_audio.a");

    let compile = Command::new("clang")
        .args([
            "-c",
            "-fobjc-arc",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
            "-mmacosx-version-min=14.2",
        ])
        .arg("-o")
        .arg(&object)
        .arg("native/system_audio_tap.m")
        .status()
        .expect("clang is required to build the system audio tap");
    assert!(
        compile.success(),
        "failed to compile native/system_audio_tap.m"
    );

    let archive_status = Command::new("libtool")
        .arg("-static")
        .arg("-o")
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("libtool is required to archive the system audio tap");
    assert!(
        archive_status.success(),
        "failed to archive the system audio tap"
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=scrib_system_audio");
    println!("cargo:rustc-link-lib=framework=CoreAudio");
    println!("cargo:rustc-link-lib=framework=AudioToolbox");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-arg=-mmacosx-version-min=14.2");

    // Plain executables have no app bundle, so embed the usage description in
    // the Mach-O. Without it the System Audio Recording prompt cannot be shown.
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    let info_plist = manifest_dir.join("native/Info.plist");
    println!("cargo:rerun-if-changed={}", info_plist.display());
    println!(
        "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        info_plist.display()
    );
}

fn link_compiler_rt() {
    let Ok(output) = Command::new("clang").arg("-print-resource-dir").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let resource_dir = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    println!("cargo:rustc-link-search=native={resource_dir}/lib/darwin");
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}
