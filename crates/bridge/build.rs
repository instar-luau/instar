//! Builds the private native configuration and frontend boundary.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));

    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );

    let libraries = output.join("libraries");
    let generated = cxx_build::bridge("src/lib.rs");

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
        .define("CMAKE_EXPORT_COMPILE_COMMANDS", "ON")
        .define("CMAKE_ARCHIVE_OUTPUT_DIRECTORY", &libraries)
        .define("CXX_BRIDGE_SOURCE", source)
        .define("CXX_INCLUDE_DIRECTORY", output.join("cxxbridge/include"))
        .define("CXX_CRATE_DIRECTORY", output.join("cxxbridge/crate"))
        .build_target("configuration");

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

    let destination = native.build();

    export_database(
        &destination.join("build/compile_commands.json"),
        &manifest.join("compile_commands.json"),
    );

    println!("cargo:rustc-link-search=native={}", libraries.display());

    for library in [
        "configuration",
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

    println!("cargo:rerun-if-changed=CMakeLists.txt");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/configuration.hpp");
    println!("cargo:rerun-if-changed=src/configuration.cpp");
    println!("cargo:rerun-if-changed=src/frontend.rs");
    println!("cargo:rerun-if-changed=src/frontend.hpp");
    println!("cargo:rerun-if-changed=src/frontend.cpp");
    println!("cargo:rerun-if-changed=vendor/luau");
}

fn export_database(source: &Path, destination: &Path) {
    let Some(resource) = clang_resource_directory() else {
        fs::copy(source, destination).expect("native compilation database must be exported");

        return;
    };

    let contents = fs::read(source).expect("native compilation database must be readable");

    let mut entries: Vec<serde_json::Value> = serde_json::from_slice(&contents)
        .expect("native compilation database must contain valid entries");

    let flag = format!("-resource-dir={}", resource.display());

    for entry in &mut entries {
        if let Some(arguments) = entry.get_mut("arguments") {
            arguments
                .as_array_mut()
                .expect("compiler arguments must be an array")
                .push(serde_json::Value::String(flag.clone()));
        } else {
            let command = entry
                .get_mut("command")
                .expect("compiler entry must contain a command");

            let original = command.as_str().expect("compiler command must be a string");
            let escaped = flag.replace('\\', "\\\\").replace('"', "\\\"");
            *command = serde_json::Value::String(format!("{original} \"{escaped}\""));
        }
    }

    let contents =
        serde_json::to_vec_pretty(&entries).expect("native tooling commands must be serializable");

    fs::write(destination, contents).expect("native compilation database must be exported");
}

fn clang_resource_directory() -> Option<PathBuf> {
    let output = match Command::new("clang").arg("-print-resource-dir").output() {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,

        Err(error) => {
            println!("cargo:warning=Clang resource directory query could not start: {error}");

            return None;
        }
    };

    if !output.status.success() {
        println!(
            "cargo:warning=Clang resource directory query failed: {}",
            output.status
        );

        return None;
    }

    let Ok(directory) = String::from_utf8(output.stdout) else {
        println!("cargo:warning=Clang resource directory query returned a non-UTF-8 path");

        return None;
    };

    let directory = PathBuf::from(directory.trim());

    if !directory.is_dir() {
        println!(
            "cargo:warning=Clang resource directory does not exist: {}",
            directory.display()
        );

        return None;
    }

    Some(directory)
}
