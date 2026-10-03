//! Builds native Luau analysis and the CXX bridge.

use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));

    for path in ["CMakeLists.txt", "src/bridge.rs", "native", "vendor/luau"] {
        println!("cargo:rerun-if-changed={path}");
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));

    let mut configuration = cmake::Config::new(&root);

    configuration
        .generator("Ninja")
        .out_dir(output.join("ninja"));

    let target = env::var("TARGET").expect("Cargo target");

    if target.contains("msvc") {
        configuration
            .define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW")
            .define("CMAKE_MSVC_RUNTIME_LIBRARY", "");
    }

    let destination = configuration.build();

    println!(
        "cargo:rustc-link-search=native={}",
        destination.join("lib").display()
    );

    let mut bridge = cxx_build::bridge("src/bridge.rs");

    bridge.std("c++17").files([
        "native/bridge.cpp",
        "native/editor.cpp",
        "native/roblox.cpp",
    ]);

    for component in [
        "Common", "Ast", "Bytecode", "Compiler", "Config", "Analysis", "VM",
    ] {
        bridge.include(root.join("vendor/luau").join(component).join("include"));
    }

    if target.contains("msvc") {
        bridge.flag("/EHsc");
    }

    bridge.compile("instar");

    for library in [
        "Luau.Analysis",
        "Luau.Config",
        "Luau.Compiler",
        "Luau.Bytecode",
        "Luau.Ast",
        "Luau.VM",
        "Luau.Common",
    ] {
        println!("cargo:rustc-link-lib=static={library}");
    }
}
