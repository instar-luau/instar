//! Builds upstream Luau and its CXX bridge.

#[cfg(windows)]
use std::path::PathBuf;

use std::{env, error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(windows)]
    #[expect(
        unsafe_code,
        reason = "CXX symlink targets require native Windows path separators"
    )]
    unsafe {
        let output = env::var_os("OUT_DIR").ok_or("Cargo did not provide OUT_DIR")?;

        env::set_var(
            "OUT_DIR",
            Path::new(&output).components().collect::<PathBuf>(),
        );
    }

    let mut native = cmake::Config::new(".");

    if env::var("CARGO_CFG_TARGET_ENV")? == "msvc" {
        let runtime = if env::var("CARGO_CFG_TARGET_FEATURE")?
            .split(',')
            .any(|feature| feature == "crt-static")
        {
            "MultiThreaded"
        } else {
            "MultiThreadedDLL"
        };

        native.define("CMAKE_MSVC_RUNTIME_LIBRARY", runtime);

        if let Some(optimization) = match native.get_profile() {
            "Release" | "RelWithDebInfo" => Some("/O2"),
            "MinSizeRel" => Some("/O1"),
            _ => None,
        } {
            native.cxxflag(optimization);
        }
    }

    let destination = native
        .define("LUAU_BUILD_CLI", "OFF")
        .define("LUAU_BUILD_TESTS", "OFF")
        .build_target("instar")
        .build();

    let mut bridge = cxx_build::bridge("src/bridge.rs");

    bridge
        .file("native/frontend.cpp")
        .file("native/index.cpp")
        .file("native/query.cpp")
        .file("native/hover.cpp")
        .file("native/types.cpp")
        .file("native/cancellation.cpp")
        .file("native/host.cpp")
        .file("native/identity.cpp")
        .file("native/configuration.cpp")
        .file("native/environment.cpp")
        .file("native/enumerations.cpp")
        .file("native/diagnostic.cpp")
        .file("native/capabilities.cpp")
        .file("native/flags.cpp")
        .file("native/instances.cpp")
        .include(".")
        .std("c++17")
        .flag_if_supported("/EHsc");

    for component in [
        "Analysis", "Ast", "CLI", "Common", "Config", "Require", "VM",
    ] {
        bridge.include(Path::new("vendor/luau").join(component).join("include"));
    }

    bridge.compile("instar");

    println!(
        "cargo:rustc-link-search=native={}",
        destination.join("build/lib").display()
    );

    for library in [
        "CLI.lib", "Analysis", "Require", "Config", "Compiler", "Bytecode", "Ast", "VM", "Common",
    ] {
        println!("cargo:rustc-link-lib=static=Luau.{library}");
    }

    for path in ["CMakeLists.txt", "src/bridge.rs", "native", "vendor/luau"] {
        println!("cargo:rerun-if-changed={path}");
    }

    Ok(())
}
