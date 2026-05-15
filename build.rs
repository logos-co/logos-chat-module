//! Generate `include/chat_module.h` from `src/lib.rs` via cbindgen.
//!
//! See `docs/ffi-tooling.md` for the design rationale.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let header_dir = crate_dir.join("include");
    let header_path = header_dir.join("chat_module.h");

    if let Err(e) = fs::create_dir_all(&header_dir) {
        println!(
            "cargo:warning=cbindgen: cannot create {}: {e}",
            header_dir.display()
        );
        return;
    }

    let config = match cbindgen::Config::from_file(crate_dir.join("cbindgen.toml")) {
        Ok(c) => c,
        Err(e) => {
            println!("cargo:warning=cbindgen: failed to load cbindgen.toml: {e}");
            return;
        }
    };

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&header_path);
        }
        Err(e) => {
            // Don't fail the build for downstream consumers who lack network
            // access to fetch git deps; just warn.
            println!("cargo:warning=cbindgen: skipping header regen: {e}");
        }
    }
}
