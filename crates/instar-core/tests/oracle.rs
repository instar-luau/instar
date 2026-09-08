#![cfg(feature = "oracle")]

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use instar_core::{
    project::Project,
    resolve::{Resolution, Resolver},
    semantics::{DeclarationKind, Namespace, Semantics},
    source::{SourceError, SourceStore},
    syntax::{Feature, Parse, ParseOptions, SyntaxKind as K},
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

static RECORDING: AtomicUsize = AtomicUsize::new(0);

fn oracle(arguments: &[&str], input: &str) -> Result<Value, Box<dyn Error>> {
    let mut child = Command::new(env!("INSTAR_UPSTREAM_ORACLE"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("oracle stdin")?
        .write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(format!(
            "oracle exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn all_features() -> ParseOptions {
    ParseOptions {
        features: [
            Feature::Classes,
            Feature::ConditionalBindings,
            Feature::IntegerLiterals,
            Feature::ValueExports,
            Feature::DebugNoInline,
            Feature::Declarations,
        ]
        .into_iter()
        .collect(),
    }
}

fn instar_parse_with(text: &str, options: ParseOptions) -> Result<Parse, Box<dyn Error>> {
    let source = SourceStore::default().open(Path::new("upstream-oracle.luau"), 1, text)?;
    Ok(Parse::with_options(source, options)?)
}

fn instar_parse(text: &str) -> Result<Parse, Box<dyn Error>> {
    instar_parse_with(text, all_features())
}

fn upstream_root() -> Result<PathBuf, Box<dyn Error>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    Ok(manifest
        .parent()
        .and_then(Path::parent)
        .ok_or("workspace root")?
        .join("vendor/luau"))
}

fn position_offset(text: &str, line: u64, column: u64) -> Option<u32> {
    let mut offset = 0usize;
    for _ in 0..line {
        offset += text.get(offset..)?.find('\n')? + 1;
    }
    u32::try_from(offset.checked_add(usize::try_from(column).ok()?)?).ok()
}

fn oracle_item_ranges(value: &Value, key: &str, text: &str) -> Option<Vec<(u32, u32)>> {
    value
        .get(key)?
        .as_array()?
        .iter()
        .map(|token| {
            let location = token.get("location")?;
            let begin = location.get("begin")?.as_array()?;
            let end = location.get("end")?.as_array()?;
            Some((
                position_offset(text, begin[0].as_u64()?, begin[1].as_u64()?)?,
                position_offset(text, end[0].as_u64()?, end[1].as_u64()?)?,
            ))
        })
        .collect()
}

fn oracle_ranges(value: &Value, text: &str) -> Option<Vec<(u32, u32)>> {
    oracle_item_ranges(value, "tokens", text)
}

fn ast_nodes(ast: &str, name: &str) -> usize {
    let marker = format!("\"type\":\"{name}\"");
    let mut locations = BTreeSet::new();
    let mut rest = ast;
    while let Some(start) = rest.find(&marker) {
        rest = &rest[start + marker.len()..];
        if let Some(location) = rest
            .find("\"location\":\"")
            .map(|start| &rest[start + 12..])
            .and_then(|value| value.find('\"').map(|end| &value[..end]))
        {
            locations.insert(location);
        }
    }
    locations.len()
}

fn ast_node_fields(ast: &str, node: &str, field: &str) -> Vec<String> {
    let marker = format!("\"type\":\"{node}\"");
    let field = format!("\"{field}\":\"");
    let mut values = BTreeSet::new();
    let mut rest = ast;
    while let Some(start) = rest.find(&marker) {
        rest = &rest[start + marker.len()..];
        let location = rest
            .find("\"location\":\"")
            .map(|start| &rest[start + 12..])
            .and_then(|value| value.find('\"').map(|end| &value[..end]));
        let value = rest
            .find(&field)
            .map(|start| &rest[start + field.len()..])
            .and_then(|value| value.find('\"').map(|end| &value[..end]));
        if let (Some(location), Some(value)) = (location, value) {
            values.insert((location, value));
        }
    }
    values
        .into_iter()
        .map(|(_, value)| value.to_owned())
        .collect()
}

fn ast_local_names(ast: &str) -> Vec<&str> {
    let marker = "\"type\":\"AstLocal\"";
    let mut bindings = BTreeSet::new();
    let mut offset = 0;
    while let Some(found) = ast[offset..].find(marker) {
        let marker_start = offset + found;
        let prefix = &ast[..marker_start];
        let name = prefix
            .rfind("\"name\":\"")
            .map(|start| &prefix[start + 8..])
            .and_then(|value| value.find('\"').map(|end| &value[..end]));
        let suffix = &ast[marker_start + marker.len()..];
        let location = suffix
            .find("\"location\":\"")
            .map(|start| &suffix[start + 12..])
            .and_then(|value| value.find('\"').map(|end| &value[..end]));
        if let (Some(name), Some(location)) = (name, location) {
            bindings.insert((name, location));
        }
        offset = marker_start + marker.len();
    }
    let mut names: Vec<_> = bindings.into_iter().map(|(name, _)| name).collect();
    names.sort_unstable();
    names
}

fn diagnostic_lines(text: &str, ranges: &[(u32, u32)]) -> Vec<(usize, usize)> {
    let line = |offset: u32| text[..offset as usize].matches('\n').count();
    let mut result: Vec<_> = ranges
        .iter()
        .map(|&(start, end)| (line(start), line(end)))
        .collect();
    result.sort_unstable();
    result.dedup();
    result
}

fn decode_hex(value: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    if !value.len().is_multiple_of(2) {
        return Err("odd hexadecimal source length".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| Ok(u8::from_str_radix(&value[index..index + 2], 16)?))
        .collect()
}

fn recorded_cases() -> Result<Vec<Value>, Box<dyn Error>> {
    let record = upstream_root()?
        .parent()
        .and_then(Path::parent)
        .ok_or("workspace root")?
        .join(format!(
            "target/upstream-oracle/parser-records-{}-{}.jsonl",
            std::process::id(),
            RECORDING.fetch_add(1, Ordering::Relaxed)
        ));
    if record.exists() {
        fs::remove_file(&record)?;
    }
    let output = Command::new(env!("INSTAR_UPSTREAM_PARSER_TESTS"))
        .arg("--no-version")
        .env("INSTAR_ORACLE_RECORD", &record)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "upstream parser tests exited with {}:\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let contents = fs::read_to_string(&record)?;
    fs::remove_file(record)?;
    contents
        .lines()
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

fn recorded_options(case: &Value) -> Result<ParseOptions, Box<dyn Error>> {
    let values = case
        .get("features")
        .and_then(Value::as_object)
        .ok_or("recorded features")?;
    let mut features = BTreeSet::new();
    for (name, feature) in [
        ("DebugLuauUserDefinedClasses", Feature::Classes),
        ("DebugLuauIfLocalSyntax", Feature::ConditionalBindings),
        ("DebugLuauNoInline", Feature::DebugNoInline),
        ("LuauExportValueSyntax", Feature::ValueExports),
        ("LuauIntegerType2", Feature::IntegerLiterals),
        ("declarations", Feature::Declarations),
    ] {
        if values.get(name).and_then(Value::as_bool) == Some(true) {
            features.insert(feature);
        }
    }
    Ok(ParseOptions { features })
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one differential pass keeps each recorded parser result and its comparisons together"
)]
fn vendored_runtime_parser_cases_match_acceptance() -> TestResult {
    let cases: Vec<_> = recorded_cases()?
        .into_iter()
        .filter(|case| case.get("mode").and_then(Value::as_str) == Some("parse"))
        .collect();
    assert!(
        cases.len() >= 400,
        "only {} parser invocations recorded",
        cases.len()
    );
    let mut differences = Vec::new();
    let mut diagnostic_differences = Vec::new();
    let mut ast_differences = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let limits = case
            .get("limits")
            .and_then(Value::as_object)
            .ok_or("recorded limits")?;
        if [
            ("LuauRecursionLimit", 1000),
            ("LuauTypeLengthLimit", 1000),
            ("LuauParseErrorLimit", 100),
        ]
        .into_iter()
        .any(|(name, default)| limits.get(name).and_then(Value::as_i64) != Some(default))
        {
            continue;
        }
        let bytes = decode_hex(
            case.get("source_hex")
                .and_then(Value::as_str)
                .ok_or("recorded source")?,
        )?;
        let upstream_ok = case
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        let Ok(input) = std::str::from_utf8(&bytes) else {
            assert!(
                !upstream_ok,
                "upstream accepted a byte-only source at #{index}"
            );
            continue;
        };
        let instar = instar_parse_with(input, recorded_options(case)?)?;
        let syntax_errors = format!("{:?}", instar.errors());
        let syntax = instar.syntax().clone();
        let mut instar_ranges: Vec<_> = instar
            .errors()
            .iter()
            .map(|error| (u32::from(error.range.start()), u32::from(error.range.end())))
            .collect();
        let semantics = Semantics::new(instar);
        instar_ranges.extend(
            semantics
                .errors()
                .iter()
                .map(|error| (u32::from(error.range.start()), u32::from(error.range.end()))),
        );
        instar_ranges.sort_unstable();
        if upstream_ok != semantics.is_complete() {
            differences.push(format!(
                "#{index}: upstream={upstream_ok}, syntax={syntax_errors}, semantics={:?}, source={input:?}",
                semantics.errors()
            ));
        } else if upstream_ok {
            let ast = case
                .get("ast")
                .and_then(Value::as_str)
                .ok_or("recorded AST")?;
            for (upstream, instar_kind) in [
                ("AstExprBinary", K::BinaryExpression),
                ("AstExprCall", K::CallExpression),
                ("AstExprGroup", K::ParenthesizedExpression),
                ("AstExprInterpString", K::InterpolationExpression),
                ("AstExprTable", K::TableExpression),
                ("AstExprTypeAssertion", K::TypeAssertionExpression),
                ("AstExprVarargs", K::VarargExpression),
                ("AstStatExpr", K::ExpressionStatement),
                ("AstStatReturn", K::ReturnStatement),
                ("AstStatWhile", K::WhileStatement),
                ("AstTypeIntersection", K::IntersectionType),
                ("AstTypeOptional", K::OptionalType),
                ("AstTypePackGeneric", K::GenericTypePack),
                ("AstTypeTable", K::TableType),
                ("AstTypeTypeof", K::TypeofType),
                ("AstTypeUnion", K::UnionType),
            ] {
                let upstream_count = ast_nodes(ast, upstream);
                let instar_count = syntax
                    .descendants()
                    .filter(|node| {
                        node.kind() == instar_kind
                            && !node
                                .ancestors()
                                .any(|ancestor| ancestor.kind() == K::AttributeArguments)
                    })
                    .count();
                if upstream_count != instar_count {
                    ast_differences.push(format!(
                        "#{index} {upstream}/{instar_kind:?}: upstream={upstream_count}, instar={instar_count}, source={input:?}"
                    ));
                }
            }
            let mut upstream_operators = BTreeMap::new();
            for operator in ast_node_fields(ast, "AstExprBinary", "op") {
                let symbol = match operator.as_str() {
                    "Add" => "+",
                    "Sub" => "-",
                    "Mul" => "*",
                    "Div" => "/",
                    "FloorDiv" => "//",
                    "Mod" => "%",
                    "Pow" => "^",
                    "Concat" => "..",
                    "CompareEq" => "==",
                    "CompareNe" => "~=",
                    "CompareLt" => "<",
                    "CompareLe" => "<=",
                    "CompareGt" => ">",
                    "CompareGe" => ">=",
                    "And" => "and",
                    "Or" => "or",
                    unknown => {
                        ast_differences.push(format!(
                            "#{index} unknown upstream binary operator {unknown:?}"
                        ));
                        continue;
                    }
                };
                *upstream_operators.entry(symbol).or_insert(0) += 1;
            }
            let mut instar_operators = BTreeMap::new();
            for operator in syntax
                .descendants()
                .filter(|node| node.kind() == K::BinaryExpression)
                .flat_map(|node| node.children_with_tokens())
                .filter_map(rowan::NodeOrToken::into_token)
                .filter(|token| matches!(token.kind(), K::Symbol | K::Keyword))
            {
                *instar_operators
                    .entry(operator.text().to_owned())
                    .or_insert(0) += 1;
            }
            if upstream_operators
                != instar_operators
                    .iter()
                    .map(|(operator, count)| (operator.as_str(), *count))
                    .collect()
            {
                ast_differences.push(format!(
                    "#{index} binary operators: upstream={upstream_operators:?}, instar={instar_operators:?}, source={input:?}"
                ));
            }

            let upstream_functions = ast_nodes(ast, "AstExprFunction");
            let instar_functions = syntax
                .descendants()
                .filter(|node| {
                    matches!(
                        node.kind(),
                        K::FunctionStatement
                            | K::FunctionExpression
                            | K::ClassMethod
                            | K::TypeFunction
                    )
                })
                .count();
            if upstream_functions != instar_functions {
                ast_differences.push(format!(
                    "#{index} functions: upstream={upstream_functions}, instar={instar_functions}, source={input:?}"
                ));
            }
            let upstream_if_expressions = ast_nodes(ast, "AstExprIfElse");
            let instar_if_expressions = syntax
                .descendants()
                .filter(|node| node.kind() == K::IfExpression)
                .count();
            if (upstream_if_expressions > 0) != (instar_if_expressions > 0) {
                ast_differences.push(format!(
                    "#{index} if expressions: upstream={upstream_if_expressions}, instar={instar_if_expressions}, source={input:?}"
                ));
            }
            let upstream_type_functions = ast_nodes(ast, "AstTypeFunction");
            let instar_type_functions = syntax
                .descendants()
                .filter(|node| matches!(node.kind(), K::FunctionType | K::ExternMethod))
                .count();
            if upstream_type_functions != instar_type_functions {
                ast_differences.push(format!(
                    "#{index} function types: upstream={upstream_type_functions}, instar={instar_type_functions}, source={input:?}"
                ));
            }
            let upstream_if_statements = ast_nodes(ast, "AstStatIf");
            let instar_if_statements = syntax
                .descendants()
                .filter(|node| node.kind() == K::IfStatement)
                .count();
            if (upstream_if_statements > 0) != (instar_if_statements > 0) {
                ast_differences.push(format!(
                    "#{index} if statements: upstream={upstream_if_statements}, instar={instar_if_statements}, source={input:?}"
                ));
            }
            let upstream_bindings = ast_local_names(ast);
            let mut instar_bindings: Vec<_> = semantics
                .declarations()
                .iter()
                .filter(|(_, declaration)| {
                    declaration.namespace == Namespace::Value
                        && matches!(
                            declaration.kind,
                            DeclarationKind::Local
                                | DeclarationKind::LocalFunction
                                | DeclarationKind::Parameter
                                | DeclarationKind::Loop
                                | DeclarationKind::ImplicitSelf
                        )
                })
                .map(|(_, declaration)| declaration.name.as_str())
                .collect();
            instar_bindings.sort_unstable();
            let conditional_bindings = case
                .get("features")
                .and_then(|features| features.get("DebugLuauIfLocalSyntax"))
                .and_then(Value::as_bool)
                == Some(true);
            if !conditional_bindings && upstream_bindings != instar_bindings {
                ast_differences.push(format!(
                    "#{index} bindings: upstream={upstream_bindings:?}, instar={instar_bindings:?}, source={input:?}"
                ));
            }
        } else {
            let mut upstream_ranges =
                oracle_item_ranges(case, "errors", input).ok_or("recorded diagnostic locations")?;
            upstream_ranges.sort_unstable();
            let upstream_lines = diagnostic_lines(input, &upstream_ranges);
            let instar_lines = diagnostic_lines(input, &instar_ranges);
            if !upstream_lines
                .iter()
                .any(|&(upstream_start, upstream_end)| {
                    instar_lines.iter().any(|&(instar_start, instar_end)| {
                        upstream_start <= instar_end.saturating_add(1)
                            && instar_start <= upstream_end.saturating_add(1)
                    })
                })
            {
                diagnostic_differences.push(format!(
                    "#{index}: upstream={upstream_ranges:?}, instar={instar_ranges:?}, source={input:?}"
                ));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "{} parser acceptance differences across {} runtime invocations:\n{}",
        differences.len(),
        cases.len(),
        differences
            .into_iter()
            .take(20)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        ast_differences.is_empty(),
        "{} parser AST differences:\n{}",
        ast_differences.len(),
        ast_differences
            .into_iter()
            .take(20)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        diagnostic_differences.is_empty(),
        "{} parser diagnostic-range differences:\n{}",
        diagnostic_differences.len(),
        diagnostic_differences
            .into_iter()
            .take(20)
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

fn source_files(root: &Path, output: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            source_files(&path, output)?;
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("lua" | "luau")
        ) {
            output.push(path);
        }
    }
    Ok(())
}

#[test]
fn vendored_conformance_files_match_acceptance() -> TestResult {
    let mut paths = Vec::new();
    source_files(&upstream_root()?.join("tests/conformance"), &mut paths)?;
    paths.sort();
    assert_ne!(paths.len(), 0);
    let mut differences = Vec::new();
    let mut non_utf8 = Vec::new();
    for path in &paths {
        let bytes = fs::read(path)?;
        let Ok(input) = std::str::from_utf8(&bytes) else {
            let source = SourceStore::default().read(path)?;
            assert!(
                matches!(source.text(), Err(SourceError::Encoding(_))),
                "{}",
                path.display()
            );
            non_utf8.push(path.file_name().ok_or("fixture file name")?.to_owned());
            continue;
        };
        let upstream = oracle(&["parse", "all"], input)?;
        let upstream_ok = upstream
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        let instar = instar_parse(input)?;
        if upstream_ok != instar.errors().is_empty() {
            differences.push(format!(
                "{}: upstream={upstream_ok}, instar={:?}",
                path.display(),
                instar.errors()
            ));
        }
    }
    assert_eq!(
        non_utf8,
        ["literals.luau", "pm.luau", "sort.luau"].map(std::ffi::OsString::from),
        "vendored byte-oriented fixture inventory changed"
    );
    assert!(
        differences.is_empty(),
        "{} conformance acceptance differences across {} files:\n{}",
        differences.len(),
        paths.len(),
        differences
            .into_iter()
            .take(20)
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

fn oracle_token_kind(kind: u64) -> Option<K> {
    match kind {
        1..=265 | 270..=277 => Some(K::Symbol),
        269 | 278 | 279 => Some(K::String),
        280 => Some(K::Number),
        281 => Some(K::Identifier),
        282 | 283 => Some(K::Comment),
        291..=311 => Some(K::Keyword),
        _ => None,
    }
}

fn token_ranges_refine(upstream: &[(u32, u32)], instar: &[(u32, u32)], source: &str) -> bool {
    let mut index = 0;
    for &(start, end) in upstream {
        while instar
            .get(index)
            .is_some_and(|&(_, token_end)| token_end <= start)
        {
            let (extra_start, extra_end) = instar[index];
            if !source[extra_start as usize..extra_end as usize]
                .bytes()
                .all(|byte| matches!(byte, b'{' | b'}'))
            {
                return false;
            }
            index += 1;
        }
        let mut cursor = start;
        while cursor < end {
            let Some(&(token_start, token_end)) = instar.get(index) else {
                return false;
            };
            if token_start != cursor || token_end > end {
                return false;
            }
            cursor = token_end;
            index += 1;
        }
    }
    instar[index..].iter().all(|&(start, end)| {
        source[start as usize..end as usize]
            .bytes()
            .all(|byte| matches!(byte, b'{' | b'}'))
    })
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "token boundary, category and literal comparisons share one recorded lexer result"
)]
fn vendored_runtime_lexer_cases_match_tokens_and_literals() -> TestResult {
    let cases: Vec<_> = recorded_cases()?
        .into_iter()
        .filter(|case| case.get("mode").and_then(Value::as_str) == Some("lex"))
        .collect();
    assert!(
        cases.len() >= 28,
        "only {} lexer invocations recorded",
        cases.len()
    );
    let mut differences = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let bytes = decode_hex(
            case.get("source_hex")
                .and_then(Value::as_str)
                .ok_or("recorded lexer source")?,
        )?;
        let Ok(input) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let profile = if case.get("integer").and_then(Value::as_bool) == Some(true) {
            "all"
        } else {
            "default"
        };
        let upstream = oracle(&["lex", profile], input)?;
        let upstream_ranges =
            oracle_ranges(&upstream, input).ok_or("invalid lexer oracle response")?;
        let instar = instar_parse(input)?;
        let tokens: Vec<_> = instar
            .syntax()
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|token| token.kind() != K::Whitespace)
            .collect();
        let instar_ranges: Vec<_> = tokens
            .iter()
            .map(|token| {
                (
                    u32::from(token.text_range().start()),
                    u32::from(token.text_range().end()),
                )
            })
            .collect();
        let interpolation = upstream
            .get("tokens")
            .and_then(Value::as_array)
            .is_some_and(|tokens| {
                tokens.iter().any(|token| {
                    matches!(
                        token.get("type").and_then(Value::as_u64),
                        Some(266..=269 | 289)
                    )
                })
            });
        let boundaries_match = if interpolation {
            token_ranges_refine(&upstream_ranges, &instar_ranges, input)
        } else {
            upstream_ranges == instar_ranges
        };
        let categories_match = upstream
            .get("tokens")
            .and_then(Value::as_array)
            .ok_or("oracle tokens")?
            .iter()
            .zip(&upstream_ranges)
            .all(|(token, range)| {
                let Some(expected) = token
                    .get("type")
                    .and_then(Value::as_u64)
                    .and_then(oracle_token_kind)
                else {
                    return true;
                };
                tokens.iter().any(|candidate| {
                    candidate.kind() == expected
                        && (
                            u32::from(candidate.text_range().start()),
                            u32::from(candidate.text_range().end()),
                        ) == *range
                })
            });

        let upstream_strings: Result<Vec<_>, Box<dyn Error>> = upstream
            .get("tokens")
            .and_then(Value::as_array)
            .ok_or("oracle tokens")?
            .iter()
            .filter(|token| {
                matches!(
                    token.get("type").and_then(Value::as_u64),
                    Some(269 | 278 | 279)
                )
            })
            .filter_map(|token| {
                token
                    .get("decoded_hex")
                    .and_then(Value::as_str)
                    .map(decode_hex)
            })
            .collect();
        let instar_strings: Vec<_> = tokens
            .iter()
            .filter(|token| token.kind() == K::String)
            .filter_map(|token| instar_core::syntax::string_bytes(token).ok())
            .collect();
        if !boundaries_match || !categories_match || upstream_strings? != instar_strings {
            differences.push(format!(
                "#{index}: upstream={upstream_ranges:?}, instar={instar_ranges:?}, source={input:?}"
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "{} lexer differences across {} runtime invocations:\n{}",
        differences.len(),
        cases.len(),
        differences
            .into_iter()
            .take(20)
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

#[test]
fn oracle_ast_shapes_cover_representative_grammar() -> TestResult {
    for (source, upstream_types, instar_kinds) in [
        (
            "local x = 1 + 2",
            &["AstStatLocal", "AstExprBinary"][..],
            &[K::LocalStatement, K::BinaryExpression][..],
        ),
        (
            "if true then f() else g() end",
            &["AstStatIf", "AstExprCall"],
            &[K::IfStatement, K::CallExpression],
        ),
        (
            "type T = {x: number}",
            &["AstStatTypeAlias", "AstTypeTable"],
            &[K::TypeAlias, K::TableType],
        ),
        (
            "local f = function(a: number): number return a end",
            &["AstExprFunction", "AstTypeReference"],
            &[K::FunctionExpression, K::TypeName],
        ),
    ] {
        let upstream = oracle(&["parse", "all"], source)?;
        assert!(
            upstream
                .get("errors")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        );
        let ast = upstream
            .get("ast")
            .and_then(Value::as_str)
            .ok_or("oracle AST")?;
        let instar = instar_parse(source)?;
        assert_eq!(instar.errors(), []);
        for upstream_type in upstream_types {
            assert!(
                ast.contains(&format!("\"type\":\"{upstream_type}\"")),
                "{upstream_type}: {ast}"
            );
        }
        for kind in instar_kinds {
            assert!(
                instar
                    .syntax()
                    .descendants()
                    .any(|node| node.kind() == *kind),
                "{kind:?}: {source}"
            );
        }
    }
    Ok(())
}

fn oracle_resolution(
    root: &Path,
    requirer: &str,
    request: &str,
) -> Result<&'static str, Box<dyn Error>> {
    let result = oracle(
        &[
            "resolve",
            root.join(requirer).to_str().ok_or("requirer path")?,
            request,
        ],
        "",
    )?;
    Ok(match result.get("status").and_then(Value::as_str) {
        Some("resolved") => "resolved",
        Some("error")
            if result
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("ambiguous")) =>
        {
            "ambiguous"
        }
        Some("error")
            if request.starts_with('@')
                || !(request.starts_with("./") || request.starts_with("../")) =>
        {
            "unsupported"
        }
        Some("missing" | "error") => "missing",
        _ => return Err(format!("invalid resolution oracle response: {result}").into()),
    })
}

fn resolution_status(resolution: &Resolution) -> &'static str {
    match resolution {
        Resolution::Resolved(_) => "resolved",
        Resolution::Missing(_) => "missing",
        Resolution::Ambiguous(_) => "ambiguous",
        Resolution::Unsupported(_) => "unsupported",
        Resolution::Dynamic => "dynamic",
        Resolution::ContextRequired => "context",
    }
}

fn instar_resolution(
    root: &Path,
    requirer: &str,
    request: &str,
) -> Result<&'static str, Box<dyn Error>> {
    let project = Project::load(root)?;
    let mut store = SourceStore::default();
    let source = format!("return require({request:?})");
    let parse = Parse::new(store.open(&root.join(requirer), 1, &source)?)?;
    let semantics = Semantics::new(parse);
    let resolver = Resolver::new(&project, &mut store)?;
    Ok(resolution_status(
        &resolver.resolve(&mut store, &semantics, 0)?,
    ))
}

#[test]
fn vendored_require_fixture_sites_match() -> TestResult {
    let root = upstream_root()?.join("tests/require/without_config");
    let project = Project::load(&root)?;
    let mut store = SourceStore::default();
    let resolver = Resolver::new(&project, &mut store)?;
    let mut paths = Vec::new();
    source_files(&root, &mut paths)?;
    paths.sort();
    let mut sites = 0;
    for path in paths {
        let source = store.read(&path)?;
        let parse = Parse::with_options(source, all_features())?;
        let semantics = Semantics::new(parse);
        let requirer = path.strip_prefix(&root)?.to_str().ok_or("requirer path")?;
        for (index, site) in semantics.requires().iter().enumerate() {
            let Some(token) = site.argument.as_ref().and_then(|argument| {
                argument
                    .descendants_with_tokens()
                    .filter_map(rowan::NodeOrToken::into_token)
                    .find(|token| token.kind() == K::String)
            }) else {
                continue;
            };
            let request = instar_core::syntax::string_bytes(&token)?;
            let request = std::str::from_utf8(&request)?;
            assert_eq!(
                resolution_status(&resolver.resolve(&mut store, &semantics, index)?),
                oracle_resolution(&root, requirer, request)?,
                "{requirer}: {request}"
            );
            sites += 1;
        }
    }
    assert_ne!(sites, 0, "no static require fixtures discovered");
    Ok(())
}

#[test]
fn vendored_require_navigation_matches() -> TestResult {
    let root = upstream_root()?.join("tests/require/without_config");
    for (requirer, request) in [
        ("module.luau", "./dependency"),
        ("module.luau", "./luau"),
        ("module.luau", "./lua"),
        ("nested/init.luau", "@self/submodule"),
        ("nested_inits/init.luau", "@self/init"),
        ("module.luau", "./ambiguous/file/dependency"),
        ("module.luau", "./ambiguous/directory/dependency"),
        ("module.luau", "./nested/init"),
        ("module.luau", "@missing"),
        ("module.luau", "bare"),
    ] {
        assert_eq!(
            instar_resolution(&root, requirer, request)?,
            oracle_resolution(&root, requirer, request)?,
            "{requirer}: {request}"
        );
    }
    Ok(())
}
