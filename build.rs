use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // Only build Swift bridge on macOS Apple Silicon
    if target_os != "macos" || target_arch != "aarch64" {
        println!("cargo:warning=Foundation Models bridge only builds on macOS aarch64, skipping");
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let swift_src = PathBuf::from("swift/foundation_models_bridge.swift");
    let lib_path = out_dir.join("libfoundation_models_bridge.a");

    // Check for macOS 26 SDK
    let sdk_output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("failed to run xcrun");
    let sdk_path = String::from_utf8(sdk_output.stdout)
        .unwrap()
        .trim()
        .to_string();

    let has_macos26_sdk = {
        let settings = format!("{}/SDKSettings.json", sdk_path);
        if let Ok(contents) = std::fs::read_to_string(&settings) {
            contents.contains("\"26.") || contents.contains("\"27.") || contents.contains("\"28.")
        } else {
            std::path::Path::new(&format!(
                "{}/System/Library/Frameworks/FoundationModels.framework",
                sdk_path
            ))
            .exists()
        }
    };

    if !has_macos26_sdk {
        println!("cargo:warning=macOS 26+ SDK not found, building Foundation Models stub");
        build_stub(&out_dir, &lib_path);
        emit_link_flags(&out_dir);
        println!("cargo:rerun-if-changed=swift/foundation_models_bridge.swift");
        return;
    }

    // Compile Swift to static library
    let status = Command::new("swiftc")
        .args([
            "-emit-library",
            "-static",
            "-module-name",
            "FoundationModelsBridge",
            "-sdk",
            &sdk_path,
            "-target",
            "arm64-apple-macos14.0",
            "-O",
            "-o",
        ])
        .arg(&lib_path)
        .arg(&swift_src)
        .status()
        .expect("failed to run swiftc — install Xcode Command Line Tools: xcode-select --install");

    if !status.success() {
        panic!("swiftc compilation failed");
    }

    emit_link_flags(&out_dir);

    // Weak-link FoundationModels so binary launches on macOS < 26 too
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,FoundationModels");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    // Swift runtime paths
    let swift_paths = [
        "/usr/lib/swift".to_string(),
        format!("{}/usr/lib/swift", sdk_path),
    ];
    for path in &swift_paths {
        if std::path::Path::new(path).exists() {
            println!("cargo:rustc-link-search=native={}", path);
        }
    }
    if let Ok(output) = Command::new("xcode-select").arg("-p").output() {
        let xcode_dev = String::from_utf8(output.stdout).unwrap().trim().to_string();
        let p = format!(
            "{}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx",
            xcode_dev
        );
        if std::path::Path::new(&p).exists() {
            println!("cargo:rustc-link-search=native={}", p);
        }
    }

    println!("cargo:rerun-if-changed=swift/foundation_models_bridge.swift");
}

fn emit_link_flags(out_dir: &Path) {
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=foundation_models_bridge");
}

fn build_stub(out_dir: &Path, lib_path: &Path) {
    let stub_src = out_dir.join("stub.c");
    std::fs::write(
        &stub_src,
        r#"
#include <stdlib.h>
#include <string.h>
static char* ms(const char* s) { char* p = malloc(strlen(s)+1); if(p) strcpy(p,s); return p; }
int fm_check_availability(char** r) { if(r) *r=ms("Foundation Models not available"); return 4; }
void fm_free_string(char* p) { if(p) free(p); }
int fm_generate_text(const char* i, const char* p, char** t, char** e) {
    if(e) *e=ms("Apple Intelligence not available"); if(t) *t=0; return -1;
}
int fm_prewarm(void) { return -1; }
"#,
    )
    .expect("write stub");

    let obj = out_dir.join("stub.o");
    Command::new("cc")
        .args(["-c", "-o"])
        .arg(&obj)
        .arg(&stub_src)
        .status()
        .expect("compile stub");
    Command::new("ar")
        .args(["rcs"])
        .arg(lib_path)
        .arg(&obj)
        .status()
        .expect("archive stub");
}
