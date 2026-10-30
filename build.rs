use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=mpv.def");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();

    if target_os == "windows" {
        // Check if an explicit LIBMPV_PATH is provided
        if let Ok(libmpv_path) = env::var("LIBMPV_PATH") {
            println!("cargo:rustc-link-search=native={}", libmpv_path);
            println!("cargo:rustc-link-lib=dylib=mpv");
        } else {
            // Check if local mpv.lib exists in crate directory
            let local_lib = Path::new(&manifest_dir).join("mpv.lib");
            if local_lib.exists() {
                println!("cargo:rustc-link-search=native={}", manifest_dir);
                println!("cargo:rustc-link-lib=dylib=mpv");
            } else {
                // We use dynamic in-process symbol resolution via GetProcAddress
                // so the crate can build and link out of the box with zero external .lib requirements!
                println!("cargo:rustc-cfg=mpv_dynamic_symbols");
            }
        }
    }
}
