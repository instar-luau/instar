//! Builds the native Luau analysis bridge and generates its Rust bindings.

use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));

    for path in [
        "CMakeLists.txt",
        "native/api.hpp",
        "native/bridge.cpp",
        "native/bridge.hpp",
        "native/editor.cpp",
        "native/editor.hpp",
        "native/roblox.cpp",
        "native/roblox.hpp",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    println!("cargo:rerun-if-changed=vendor/luau");

    let bindings = bindgen::Builder::default()
        .header(root.join("native/api.hpp").display().to_string())
        .allowlist_file(".*native.*")
        .rustified_enum(".*")
        .clang_arg("-x")
        .clang_arg("c")
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("generate native bindings");

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));

    bindings
        .write_to_file(output.join("bindings.rs"))
        .expect("write native bindings");

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

    println!("cargo:rustc-link-lib=static=Instar");

    for library in [
        "Luau.Analysis",
        "Luau.CLI.lib",
        "Luau.Config",
        "Luau.Compiler",
        "Luau.Bytecode",
        "Luau.Ast",
        "Luau.VM",
        "Luau.Common",
    ] {
        println!("cargo:rustc-link-lib=static={library}");
    }

    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
