//! Builds the private captured-analysis and configuration boundary.

use std::{env, path::PathBuf};

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));

    let libraries = output.join("libraries");
    let generated = cxx_build::bridge("src/boundary/mod.rs");

    let source = generated
        .get_files()
        .next()
        .expect("CXX must generate a bridge source");

    let mut native = cmake::Config::new(".");

    native
        .generator("Ninja")
        .define("LUAU_BUILD_CLI", "OFF")
        .define("LUAU_BUILD_TESTS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_ARCHIVE_OUTPUT_DIRECTORY", &libraries)
        .define("CXX_BRIDGE_SOURCE", source)
        .define("CXX_INCLUDE_DIRECTORY", output.join("cxxbridge/include"))
        .define("CXX_CRATE_DIRECTORY", output.join("cxxbridge/crate"))
        .build_target("bridge");

    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();

    let runtime = if features.split(',').any(|feature| feature == "crt-static") {
        "MultiThreaded"
    } else {
        "MultiThreadedDLL"
    };

    native.define("CMAKE_MSVC_RUNTIME_LIBRARY", runtime);

    for profile in ["DEBUG", "RELEASE", "RELWITHDEBINFO", "MINSIZEREL"] {
        native.define(
            format!("CMAKE_ARCHIVE_OUTPUT_DIRECTORY_{profile}"),
            &libraries,
        );
    }

    native.build();

    println!("cargo:rustc-link-search=native={}", libraries.display());

    for library in [
        "bridge",
        "Luau.Analysis",
        "Luau.Config",
        "Luau.Compiler",
        "Luau.Ast",
        "Luau.Bytecode",
        "Luau.VM",
        "Luau.Common",
    ] {
        println!("cargo:rustc-link-lib=static={library}");
    }

    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");
    println!("cargo:rerun-if-changed=CMakeLists.txt");
    println!("cargo:rerun-if-changed=src/boundary");
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=src/worker/operation.rs");
    println!("cargo:rerun-if-changed=vendor/luau");
}
