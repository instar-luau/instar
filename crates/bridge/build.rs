//! Builds upstream Luau and its CXX bridge.

#[cfg(windows)]
use std::path::PathBuf;

use std::{collections::BTreeMap, env, error::Error, fmt::Write as _, fs, path::Path};

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

    generate()?;

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
        .file("native/cancellation.cpp")
        .file("native/host.cpp")
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

struct Builtins<'source> {
    remaining: &'source str,
    constants: BTreeMap<&'source str, &'source str>,
    selectors: Vec<&'source str>,
}

impl Builtins<'_> {
    fn literal(&mut self) -> Result<String, Box<dyn Error>> {
        let name = identifier(&mut self.remaining)?;

        let source = self
            .constants
            .get(name)
            .ok_or_else(|| format!("unsupported Luau builtin declaration constant: {name}"))?;

        Ok(format!("{source:?}"))
    }

    fn condition(&mut self) -> Result<String, Box<dyn Error>> {
        let mut conditions = Vec::new();

        loop {
            consume(&mut self.remaining, "FFlag::")?;

            let name = identifier(&mut self.remaining)?;

            let index =
                if let Some(index) = self.selectors.iter().position(|selector| *selector == name) {
                    index
                } else {
                    self.selectors.push(name);

                    self.selectors.len() - 1
                };

            conditions.push(format!("selectors[{index}]"));

            if !self.remaining.trim_start().starts_with("&&") {
                break;
            }

            consume(&mut self.remaining, "&&")?;
        }

        Ok(conditions.join(" && "))
    }

    fn append(&mut self) -> Result<String, Box<dyn Error>> {
        let braced = self.remaining.trim_start().starts_with('{');

        if braced {
            consume(&mut self.remaining, "{")?;
        }

        consume(&mut self.remaining, "result")?;
        consume(&mut self.remaining, "+=")?;

        let literal = self.literal()?;

        consume(&mut self.remaining, ";")?;

        if braced {
            consume(&mut self.remaining, "}")?;
        }

        Ok(format!("result.push_str({literal});\n"))
    }

    fn assembly(&mut self) -> Result<String, Box<dyn Error>> {
        consume(&mut self.remaining, "{")?;
        consume(&mut self.remaining, "std::string result")?;
        consume(&mut self.remaining, "=")?;

        let condition = self.condition()?;

        consume(&mut self.remaining, "?")?;

        let consequence = self.literal()?;

        consume(&mut self.remaining, ":")?;

        let alternative = self.literal()?;

        consume(&mut self.remaining, ";")?;

        let mut generated = format!(
            "fn assemble(selectors: &[bool]) -> String {{\n\
             let mut result = String::from(if {condition} {{ {consequence} }} else {{ {alternative} }});\n"
        );

        while !self.remaining.trim_start().starts_with("return") {
            if self.remaining.trim_start().starts_with("if") {
                consume(&mut self.remaining, "if")?;
                consume(&mut self.remaining, "(")?;

                let condition = self.condition()?;

                consume(&mut self.remaining, ")")?;

                let consequence = self.append()?;

                writeln!(generated, "if {condition} {{\n{consequence}}}")?;

                if self.remaining.trim_start().starts_with("else") {
                    consume(&mut self.remaining, "else")?;

                    let alternative = self.append()?;

                    writeln!(generated, "else {{\n{alternative}}}")?;
                }
            } else {
                generated.push_str(&self.append()?);
            }
        }

        consume(&mut self.remaining, "return result;")?;
        consume(&mut self.remaining, "}")?;
        generated.push_str("result\n}\n");

        Ok(generated)
    }
}

fn generate() -> Result<(), Box<dyn Error>> {
    let source = fs::read_to_string("vendor/luau/Analysis/src/EmbeddedBuiltinDefinitions.cpp")?;
    let source = source.replace("\r\n", "\n");
    let signature = "std::string getBuiltinDefinitionSource()";

    if source.matches(signature).count() != 1 {
        return Err("cannot locate a unique Luau builtin declaration assembly".into());
    }

    let (prefix, remaining) = source
        .split_once(signature)
        .ok_or("cannot locate Luau builtin declaration assembly")?;

    let (preamble, mut definitions) = prefix
        .split_once("namespace Luau\n{")
        .ok_or("unsupported Luau builtin declaration namespace")?;

    for line in preamble
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("//") || line == "#include \"Luau/BuiltinDefinitions.h\"" {
            continue;
        }

        let mut declaration = line
            .strip_prefix("LUAU_FASTFLAG(")
            .or_else(|| line.strip_prefix("LUAU_FASTFLAGVARIABLE("))
            .ok_or("unsupported Luau builtin declaration preamble")?;

        identifier(&mut declaration)?;
        consume(&mut declaration, ")")?;

        if !declaration.trim().is_empty() {
            return Err("unsupported Luau builtin flag declaration".into());
        }
    }

    let mut constants = BTreeMap::new();

    while !definitions.trim().is_empty() {
        definitions = definitions.trim_start();

        let prefix = [
            "static constexpr const char* ",
            "static const char* const ",
            "static const char* ",
        ]
        .into_iter()
        .find(|prefix| definitions.starts_with(*prefix))
        .ok_or("unsupported Luau builtin declaration constant")?;

        consume(&mut definitions, prefix)?;

        let name = identifier(&mut definitions)?;

        consume(&mut definitions, "=")?;
        consume(&mut definitions, "R\"")?;

        let (delimiter, remaining) = definitions
            .split_once('(')
            .ok_or("unsupported Luau builtin raw string")?;

        if delimiter.is_empty()
            || delimiter.len() > 16
            || !delimiter
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err("unsupported Luau builtin raw string delimiter".into());
        }

        let ending = format!("){delimiter}\"");

        let (literal, remaining) = remaining
            .split_once(&ending)
            .ok_or("unterminated Luau builtin raw string")?;

        if literal.contains('\0') || constants.insert(name, literal).is_some() {
            return Err("invalid or duplicate Luau builtin declaration constant".into());
        }

        definitions = remaining;
        consume(&mut definitions, ";")?;
    }

    let mut builtins = Builtins {
        remaining,
        constants,
        selectors: Vec::new(),
    };

    let assembly = builtins.assembly()?;

    let metadata = format!(
        "const SELECTORS: [&str; {}] = {:?};\n",
        builtins.selectors.len(),
        builtins.selectors
    );

    let output = env::var_os("OUT_DIR").ok_or("Cargo did not provide OUT_DIR")?;

    fs::write(Path::new(&output).join("builtins.rs"), metadata + &assembly)?;

    Ok(())
}

fn identifier<'source>(source: &mut &'source str) -> Result<&'source str, Box<dyn Error>> {
    *source = source.trim_start();

    let end = source
        .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .unwrap_or(source.len());

    let (name, remaining) = source.split_at(end);

    if name.is_empty() || name.starts_with(|character: char| character.is_ascii_digit()) {
        return Err("unsupported Luau builtin declaration identifier".into());
    }

    *source = remaining;

    Ok(name)
}

fn consume(source: &mut &str, expected: &str) -> Result<(), Box<dyn Error>> {
    *source = source.trim_start().strip_prefix(expected).ok_or_else(|| {
        format!("unsupported Luau builtin declaration assembly: expected {expected:?}")
    })?;

    Ok(())
}
