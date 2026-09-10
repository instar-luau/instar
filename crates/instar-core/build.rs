use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    println!("cargo:rerun-if-changed=native");
    println!(
        "cargo:rerun-if-changed={}",
        root.join("../../vendor/luau").display()
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let destination = cmake::Config::new(root.join("native"))
        .out_dir(output.join("native"))
        .build();
    println!(
        "cargo:rustc-link-search=native={}",
        destination.join("lib").display()
    );
    for library in [
        "Instar",
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
    let target = env::var("TARGET").expect("Cargo target");
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
