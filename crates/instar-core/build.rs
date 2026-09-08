use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_ORACLE");
    if env::var_os("CARGO_FEATURE_ORACLE").is_none() {
        return;
    }

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let workspace = manifest
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let output = workspace.join("target/upstream-oracle");
    let vendor = workspace.join("vendor/luau");

    println!("cargo:rerun-if-changed=tests/oracle/CMakeLists.txt");
    println!("cargo:rerun-if-changed=tests/oracle/oracle.cpp");
    println!("cargo:rerun-if-changed=tests/oracle/recorder.cpp");
    println!("cargo:rerun-if-changed=tests/oracle/recorder.h");
    println!("cargo:rerun-if-changed={}", vendor.join("Ast").display());
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("Require").display()
    );
    println!("cargo:rerun-if-changed={}", vendor.join("CLI").display());
    println!("cargo:rerun-if-changed={}", vendor.join("tests").display());

    let destination = cmake::Config::new(manifest.join("tests/oracle"))
        .out_dir(&output)
        .define("CMAKE_EXPORT_COMPILE_COMMANDS", "ON")
        .build();

    let compile_flags = [
        "-xc++".to_owned(),
        "-std=c++17".to_owned(),
        format!("-I{}", vendor.join("Common/include").display()),
        format!("-I{}", vendor.join("Ast/include").display()),
        format!("-I{}", vendor.join("Analysis/include").display()),
        format!("-I{}", vendor.join("Config/include").display()),
        format!("-I{}", vendor.join("CLI/include").display()),
        format!("-I{}", vendor.join("Require/include").display()),
        format!("-I{}", vendor.join("VM/include").display()),
    ];
    fs::create_dir_all(&output).expect("oracle output directory");
    fs::write(output.join("compile_flags.txt"), compile_flags.join("\n"))
        .expect("clangd compile flags");

    let executable = destination.join("bin").join(if cfg!(windows) {
        "instar-upstream-oracle.exe"
    } else {
        "instar-upstream-oracle"
    });
    println!(
        "cargo:rustc-env=INSTAR_UPSTREAM_ORACLE={}",
        executable.display()
    );
    let parser_tests = destination.join("bin").join(if cfg!(windows) {
        "instar-upstream-parser-tests.exe"
    } else {
        "instar-upstream-parser-tests"
    });
    println!(
        "cargo:rustc-env=INSTAR_UPSTREAM_PARSER_TESTS={}",
        parser_tests.display()
    );
}
