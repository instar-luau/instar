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
    source::SourceStore,
    syntax::{EntryPoint, Feature, Parse, ParseOptions, SyntaxKind as K},
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

static RECORDING: AtomicUsize = AtomicUsize::new(0);

fn oracle(arguments: &[&str], input: impl AsRef<[u8]>) -> Result<Value, Box<dyn Error>> {
    let mut child = Command::new(env!("INSTAR_ORACLE"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("oracle stdin")?
        .write_all(input.as_ref())?;
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
        recursion_limit: None,
        type_length_limit: None,
        error_limit: None,
    }
}

fn instar_parse_with(
    text: impl AsRef<[u8]>,
    options: ParseOptions,
    entry: EntryPoint,
) -> Result<Parse, Box<dyn Error>> {
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(text.as_ref())?;
    let source = SourceStore::default().read(file.path())?;
    Ok(Parse::entry(source, options, entry)?)
}

fn instar_parse(text: impl AsRef<[u8]>) -> Result<Parse, Box<dyn Error>> {
    instar_parse_with(text, all_features(), EntryPoint::Module)
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

fn ast_objects(ast: &Value) -> Vec<&serde_json::Map<String, Value>> {
    let mut pending = vec![ast];
    let mut objects = Vec::new();
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(object) => {
                if let Some(kind) = object.get("type") {
                    assert!(
                        kind.is_string(),
                        "AST discriminator overwritten by a payload: {object:?}"
                    );
                    assert_ne!(
                        kind, "UnhandledAstNode",
                        "unhandled upstream AST node: {object:?}"
                    );
                }
                objects.push(object);
                pending.extend(object.values());
            }
            Value::Array(values) => pending.extend(values),
            _ => {}
        }
    }
    objects
}

fn ast_nodes(ast: &Value, name: &str) -> usize {
    ast_objects(ast)
        .into_iter()
        .filter(|object| object.get("type").and_then(Value::as_str) == Some(name))
        .map(|object| {
            object
                .get("location")
                .and_then(Value::as_str)
                .expect("AST node location")
        })
        .collect::<BTreeSet<_>>()
        .len()
}

fn ast_node_fields(ast: &Value, node: &str, field: &str) -> Vec<String> {
    ast_objects(ast)
        .into_iter()
        .filter(|object| object.get("type").and_then(Value::as_str) == Some(node))
        .map(|object| {
            (
                object
                    .get("location")
                    .and_then(Value::as_str)
                    .expect("AST node location"),
                object
                    .get(field)
                    .and_then(Value::as_str)
                    .expect("AST field"),
            )
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(_, value)| value.to_owned())
        .collect()
}

fn ast_span(
    object: &serde_json::Map<String, Value>,
    source: &str,
) -> Result<(u32, u32), Box<dyn Error>> {
    let location = object
        .get("location")
        .and_then(Value::as_str)
        .ok_or("AST location")?;
    let (begin, end) = location.split_once(" - ").ok_or("AST location separator")?;
    let offset = |position: &str| -> Result<u32, Box<dyn Error>> {
        let (line, column) = position.split_once(',').ok_or("AST position")?;
        position_offset(source, line.parse()?, column.parse()?)
            .ok_or_else(|| "AST position outside source".into())
    };
    Ok((offset(begin)?, offset(end)?))
}

fn number_values_match(ast: &Value, parse: &Parse) -> Result<bool, Box<dyn Error>> {
    use instar_core::syntax::{NumberValue, number_value};
    let source = parse.syntax().to_string();
    let mut expected = BTreeMap::new();
    for object in ast_objects(ast) {
        let value = match object.get("type").and_then(Value::as_str) {
            Some("AstExprConstantNumber") => {
                serde_json::json!({"float_bits": object.get("bits").and_then(Value::as_u64).ok_or("upstream float bits")?})
            }
            Some("AstExprConstantInteger") => {
                serde_json::json!({"integer": object.get("value").and_then(Value::as_i64).ok_or("upstream integer value")?})
            }
            _ => continue,
        };
        let span = ast_span(object, &source)?;
        if let Some(previous) = expected.insert(span, value.clone()) {
            assert_eq!(
                previous, value,
                "inconsistent upstream numeral identity at {span:?}"
            );
        }
    }
    let mut actual = BTreeMap::new();
    for token in parse
        .syntax()
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|token| token.kind() == K::Number)
    {
        let value = match number_value(token.text())
            .ok_or("instar numeral decoding")?
            .value
        {
            NumberValue::Float(value) => serde_json::json!({"float_bits": value.to_bits()}),
            NumberValue::Integer(value) => serde_json::json!({"integer": value}),
        };
        actual.insert(
            (
                u32::from(token.text_range().start()),
                u32::from(token.text_range().end()),
            ),
            value,
        );
    }
    Ok(expected == actual)
}

fn ast_local_names(ast: &Value) -> Vec<String> {
    let mut names = ast_node_fields(ast, "AstLocal", "name");
    names.sort_unstable();
    names
}

#[allow(dead_code)]
#[expect(
    clippy::match_same_arms,
    reason = "Luau AST aliases intentionally share syntax nodes"
)]
fn upstream_kind(name: &str) -> Option<&'static str> {
    Some(match name {
        "AstStatBlock" => "Block",
        "AstStatIf" => "IfStatement",
        "AstStatWhile" => "WhileStatement",
        "AstStatRepeat" => "RepeatStatement",
        "AstStatBreak" => "BreakStatement",
        "AstStatContinue" => "ContinueStatement",
        "AstStatReturn" => "ReturnStatement",
        "AstStatExpr" => "ExpressionStatement",
        "AstStatLocal" => "LocalStatement",
        "AstStatFor" => "ForStatement",
        "AstStatForIn" => "ForStatement",
        "AstStatAssign" => "AssignmentStatement",
        "AstStatCompoundAssign" => "AssignmentStatement",
        "AstStatFunction" => "FunctionStatement",
        "AstStatLocalFunction" => "FunctionStatement",
        "AstStatTypeAlias" => "TypeAlias",
        "AstStatTypeFunction" => "TypeFunction",
        "AstStatDeclareFunction" => "DeclareFunction",
        "AstStatDeclareGlobal" => "DeclareGlobal",
        "AstStatClass" => "ClassStatement",
        "AstStatDeclareExternType" => "DeclareExtern",
        "AstExprGroup" => "ParenthesizedExpression",
        "AstExprConstantNil"
        | "AstExprConstantBool"
        | "AstExprConstantNumber"
        | "AstExprConstantInteger"
        | "AstExprConstantString" => "LiteralExpression",
        "AstExprVarargs" => "VarargExpression",
        "AstExprCall" => "CallExpression",
        "AstExprIndexName" | "AstExprIndexExpr" => "IndexExpression",
        "AstExprTable" => "TableExpression",
        "AstExprUnary" => "UnaryExpression",
        "AstExprBinary" => "BinaryExpression",
        "AstExprTypeAssertion" => "TypeAssertionExpression",
        "AstExprIfElse" => "IfExpression",
        "AstExprInterpString" => "InterpolationExpression",
        "AstExprInstantiate" => "InstantiationExpression",
        "AstTypeTable" => "TableType",
        "AstTypeFunction" => "FunctionType",
        "AstTypeTypeof" => "TypeofType",
        "AstTypeOptional" => "OptionalType",
        "AstTypeUnion" => "UnionType",
        "AstTypeIntersection" => "IntersectionType",
        "AstTypePackGeneric" => "GenericTypePack",
        _ => return None,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalIdentity {
    name: String,
    declaration_span: (u32, u32),
}

/// The deliberately small common semantic vocabulary used by both AST projections.
/// Every child role remains named; arrays retain source order.
#[derive(Clone, Debug, Eq, PartialEq)]
enum SemanticNode {
    Module {
        span: (u32, u32),
        block: Box<Self>,
    },
    Block {
        span: (u32, u32),
        statements: Vec<Self>,
    },
    Return {
        span: (u32, u32),
        values: Vec<Self>,
    },
    Local {
        span: (u32, u32),
        bindings: Vec<Self>,
        values: Vec<Self>,
    },
    Binding {
        span: (u32, u32),
        name: String,
        constant: bool,
        annotation: Option<Box<Self>>,
    },
    Assignment {
        span: (u32, u32),
        targets: Vec<Self>,
        values: Vec<Self>,
        operator: Option<String>,
    },
    While {
        span: (u32, u32),
        condition: Box<Self>,
        body: Box<Self>,
        has_do: bool,
    },
    Repeat {
        span: (u32, u32),
        body: Box<Self>,
        condition: Box<Self>,
    },
    Break {
        span: (u32, u32),
    },
    Continue {
        span: (u32, u32),
    },
    Vararg {
        span: (u32, u32),
    },
    ExpressionStatement {
        span: (u32, u32),
        expression: Box<Self>,
    },
    Binary {
        span: (u32, u32),
        operator: String,
        left: Box<Self>,
        right: Box<Self>,
    },
    Unary {
        span: (u32, u32),
        operator: String,
        operand: Box<Self>,
    },
    Group {
        span: (u32, u32),
        expression: Box<Self>,
    },
    Nil {
        span: (u32, u32),
    },
    Bool {
        span: (u32, u32),
        value: bool,
    },
    Float {
        span: (u32, u32),
        bits: u64,
    },
    Integer {
        span: (u32, u32),
        bits: i64,
    },
    String {
        span: (u32, u32),
        bytes: Vec<u8>,
    },
    Reference {
        span: (u32, u32),
        name: String,
        local: Option<LocalIdentity>,
    },
    Call {
        span: (u32, u32),
        callee: Box<Self>,
        arguments: Vec<Self>,
    },
    Field {
        span: (u32, u32),
        base: Box<Self>,
        field: String,
    },
    Index {
        span: (u32, u32),
        base: Box<Self>,
        index: Box<Self>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Unsupported(String);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for Unsupported {}

type AstTopology = SemanticNode;
type CanonicalResult<T> = Result<T, Unsupported>;
type AstPayloads = Vec<(u32, u32, String, Value)>;

fn unsupported(kind: &str, field: &str, path: &str) -> Unsupported {
    Unsupported(format!(
        "Unsupported(kind={kind}, field={field}, path={path})"
    ))
}

fn exact_fields(
    object: &serde_json::Map<String, Value>,
    kind: &str,
    allowed: &[&str],
    path: &str,
) -> CanonicalResult<()> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(unsupported(kind, field, path));
    }
    if let Some(field) = allowed.iter().find(|field| !object.contains_key(**field)) {
        return Err(unsupported(kind, field, path));
    }
    Ok(())
}

fn field<'a>(
    object: &'a serde_json::Map<String, Value>,
    kind: &str,
    name: &str,
    path: &str,
) -> CanonicalResult<&'a Value> {
    object
        .get(name)
        .ok_or_else(|| unsupported(kind, name, path))
}

fn json_bytes(value: &str, kind: &str, path: &str) -> CanonicalResult<Vec<u8>> {
    value
        .chars()
        .map(|character| {
            u8::try_from(u32::from(character))
                .map_err(|_| unsupported(kind, "value(non-byte)", path))
        })
        .collect()
}

fn upstream_operator(kind: &str, operator: &str, path: &str) -> CanonicalResult<String> {
    let operator = match (kind, operator) {
        ("AstExprBinary", "Add") => "+",
        ("AstExprBinary", "Sub") | ("AstExprUnary", "Minus") => "-",
        ("AstExprBinary", "Mul") => "*",
        ("AstExprBinary", "Div") => "/",
        ("AstExprBinary", "FloorDiv") => "//",
        ("AstExprBinary", "Mod") => "%",
        ("AstExprBinary", "Pow") => "^",
        ("AstExprBinary", "Concat") => "..",
        ("AstExprBinary", "CompareEq") => "==",
        ("AstExprBinary", "CompareNe") => "~=",
        ("AstExprBinary", "CompareLt") => "<",
        ("AstExprBinary", "CompareLe") => "<=",
        ("AstExprBinary", "CompareGt") => ">",
        ("AstExprBinary", "CompareGe") => ">=",
        ("AstExprBinary", "And") => "and",
        ("AstExprBinary", "Or") => "or",
        ("AstExprUnary", "Not") => "not",
        ("AstExprUnary", "Len") => "#",
        _ => return Err(unsupported(kind, "op", path)),
    };
    Ok(operator.to_owned())
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive upstream field validation keeps each supported node schema explicit"
)]
fn upstream_node(value: &Value, source: &str, path: &str) -> CanonicalResult<SemanticNode> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported("non-object", "node", path))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported("unknown", "type", path))?;
    let span = ast_span(object, source).map_err(|_| unsupported(kind, "location", path))?;
    let node = |name: &str| field(object, kind, name, path);
    let child = |name: &str| upstream_node(node(name)?, source, &format!("{path}.{name}"));
    let children = |name: &str| -> CanonicalResult<Vec<SemanticNode>> {
        node(name)?
            .as_array()
            .ok_or_else(|| unsupported(kind, name, path))?
            .iter()
            .enumerate()
            .map(|(index, value)| upstream_node(value, source, &format!("{path}.{name}[{index}]")))
            .collect()
    };
    match kind {
        "AstStatBlock" => {
            exact_fields(object, kind, &["type", "location", "hasEnd", "body"], path)?;
            if !node("hasEnd")?.is_boolean() {
                return Err(unsupported(kind, "hasEnd", path));
            }
            Ok(SemanticNode::Block {
                span,
                statements: children("body")?,
            })
        }
        "AstStatReturn" => {
            exact_fields(object, kind, &["type", "location", "list"], path)?;
            Ok(SemanticNode::Return {
                span,
                values: children("list")?,
            })
        }
        "AstStatLocal" => {
            exact_fields(object, kind, &["type", "location", "vars", "values"], path)?;
            Ok(SemanticNode::Local {
                span,
                bindings: children("vars")?,
                values: children("values")?,
            })
        }
        "AstLocal" => {
            exact_fields(
                object,
                kind,
                &["type", "location", "name", "isConst", "luauType"],
                path,
            )?;
            Ok(SemanticNode::Binding {
                span,
                name: node("name")?
                    .as_str()
                    .ok_or_else(|| unsupported(kind, "name", path))?
                    .to_owned(),
                constant: node("isConst")?
                    .as_bool()
                    .ok_or_else(|| unsupported(kind, "isConst", path))?,
                annotation: if node("luauType")?.is_null() {
                    None
                } else {
                    Some(Box::new(child("luauType")?))
                },
            })
        }
        "AstStatAssign" => {
            exact_fields(object, kind, &["type", "location", "vars", "values"], path)?;
            Ok(SemanticNode::Assignment {
                span,
                targets: children("vars")?,
                values: children("values")?,
                operator: None,
            })
        }
        "AstStatCompoundAssign" => {
            exact_fields(
                object,
                kind,
                &["type", "location", "op", "var", "value"],
                path,
            )?;
            let operator = node("op")?
                .as_str()
                .ok_or_else(|| unsupported(kind, "op", path))?;
            Ok(SemanticNode::Assignment {
                span,
                targets: vec![child("var")?],
                values: vec![child("value")?],
                operator: Some(upstream_operator("AstExprBinary", operator, path)?),
            })
        }
        "AstStatWhile" => {
            exact_fields(
                object,
                kind,
                &["type", "location", "condition", "body", "hasDo"],
                path,
            )?;
            Ok(SemanticNode::While {
                span,
                condition: Box::new(child("condition")?),
                body: Box::new(child("body")?),
                has_do: node("hasDo")?
                    .as_bool()
                    .ok_or_else(|| unsupported(kind, "hasDo", path))?,
            })
        }
        "AstStatRepeat" => {
            exact_fields(
                object,
                kind,
                &["type", "location", "condition", "body"],
                path,
            )?;
            Ok(SemanticNode::Repeat {
                span,
                body: Box::new(child("body")?),
                condition: Box::new(child("condition")?),
            })
        }
        "AstStatBreak" | "AstStatContinue" | "AstExprVarargs" => {
            exact_fields(object, kind, &["type", "location"], path)?;
            Ok(match kind {
                "AstStatBreak" => SemanticNode::Break { span },
                "AstStatContinue" => SemanticNode::Continue { span },
                _ => SemanticNode::Vararg { span },
            })
        }
        "AstStatExpr" => {
            exact_fields(object, kind, &["type", "location", "expr"], path)?;
            Ok(SemanticNode::ExpressionStatement {
                span,
                expression: Box::new(child("expr")?),
            })
        }
        "AstExprBinary" => {
            exact_fields(
                object,
                kind,
                &["type", "location", "op", "left", "right"],
                path,
            )?;
            let operator = node("op")?
                .as_str()
                .ok_or_else(|| unsupported(kind, "op", path))?;
            Ok(SemanticNode::Binary {
                span,
                operator: upstream_operator(kind, operator, path)?,
                left: Box::new(child("left")?),
                right: Box::new(child("right")?),
            })
        }
        "AstExprUnary" => {
            exact_fields(object, kind, &["type", "location", "op", "expr"], path)?;
            let operator = node("op")?
                .as_str()
                .ok_or_else(|| unsupported(kind, "op", path))?;
            Ok(SemanticNode::Unary {
                span,
                operator: upstream_operator(kind, operator, path)?,
                operand: Box::new(child("expr")?),
            })
        }
        "AstExprGroup" => {
            exact_fields(object, kind, &["type", "location", "expr"], path)?;
            Ok(SemanticNode::Group {
                span,
                expression: Box::new(child("expr")?),
            })
        }
        "AstExprConstantNil" => {
            exact_fields(object, kind, &["type", "location"], path)?;
            Ok(SemanticNode::Nil { span })
        }
        "AstExprConstantBool" => {
            exact_fields(object, kind, &["type", "location", "value"], path)?;
            let value = node("value")?
                .as_bool()
                .ok_or_else(|| unsupported(kind, "value", path))?;
            Ok(SemanticNode::Bool { span, value })
        }
        "AstExprConstantNumber" => {
            exact_fields(object, kind, &["type", "location", "value", "bits"], path)?;
            let bits = node("bits")?
                .as_u64()
                .ok_or_else(|| unsupported(kind, "bits", path))?;
            Ok(SemanticNode::Float { span, bits })
        }
        "AstExprConstantInteger" => {
            exact_fields(object, kind, &["type", "location", "value"], path)?;
            let bits = node("value")?
                .as_i64()
                .ok_or_else(|| unsupported(kind, "value", path))?;
            Ok(SemanticNode::Integer { span, bits })
        }
        "AstExprConstantString" => {
            exact_fields(object, kind, &["type", "location", "value"], path)?;
            let value = node("value")?
                .as_str()
                .ok_or_else(|| unsupported(kind, "value", path))?;
            Ok(SemanticNode::String {
                span,
                bytes: json_bytes(value, kind, path)?,
            })
        }
        "AstExprGlobal" => {
            exact_fields(object, kind, &["type", "location", "global"], path)?;
            let name = node("global")?
                .as_str()
                .ok_or_else(|| unsupported(kind, "global", path))?;
            Ok(SemanticNode::Reference {
                span,
                name: name.to_owned(),
                local: None,
            })
        }
        "AstExprLocal" => {
            exact_fields(object, kind, &["type", "location", "local"], path)?;
            let local = node("local")?
                .as_object()
                .ok_or_else(|| unsupported(kind, "local", path))?;
            exact_fields(
                local,
                "AstLocal",
                &["luauType", "name", "isConst", "type", "location"],
                &format!("{path}.local"),
            )?;
            if local.get("type").and_then(Value::as_str) != Some("AstLocal")
                || !local.get("isConst").is_some_and(Value::is_boolean)
                || !local.get("luauType").is_some_and(Value::is_null)
            {
                return Err(unsupported(
                    "AstLocal",
                    "metadata",
                    &format!("{path}.local"),
                ));
            }
            let name = local
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| unsupported("AstLocal", "name", path))?;
            let declaration_span =
                ast_span(local, source).map_err(|_| unsupported("AstLocal", "location", path))?;
            Ok(SemanticNode::Reference {
                span,
                name: name.to_owned(),
                local: Some(LocalIdentity {
                    name: name.to_owned(),
                    declaration_span,
                }),
            })
        }
        "AstExprCall" => {
            exact_fields(
                object,
                kind,
                &["type", "location", "func", "args", "self", "argLocation"],
                path,
            )?;
            if node("self")?.as_bool() != Some(false) || !node("argLocation")?.is_string() {
                return Err(unsupported(kind, "self/argLocation", path));
            }
            Ok(SemanticNode::Call {
                span,
                callee: Box::new(child("func")?),
                arguments: children("args")?,
            })
        }
        "AstExprIndexName" => {
            exact_fields(
                object,
                kind,
                &["type", "location", "expr", "index", "indexLocation", "op"],
                path,
            )?;
            if node("op")?.as_str() != Some(".") || !node("indexLocation")?.is_string() {
                return Err(unsupported(kind, "op/indexLocation", path));
            }
            let field_name = node("index")?
                .as_str()
                .ok_or_else(|| unsupported(kind, "index", path))?;
            Ok(SemanticNode::Field {
                span,
                base: Box::new(child("expr")?),
                field: field_name.to_owned(),
            })
        }
        "AstExprIndexExpr" => {
            exact_fields(object, kind, &["type", "location", "expr", "index"], path)?;
            Ok(SemanticNode::Index {
                span,
                base: Box::new(child("expr")?),
                index: Box::new(child("index")?),
            })
        }
        _ => Err(unsupported(kind, "node", path)),
    }
}

fn ast_topology(ast: &Value, source: &str) -> CanonicalResult<AstTopology> {
    let (wrapper, supplement) = if let Some(values) = ast.as_array() {
        if values.len() != 2 {
            return Err(unsupported("wrapper", "length", "$"));
        }
        (&values[0], Some(&values[1]))
    } else {
        (ast, None)
    };
    if supplement.is_some_and(|value| value.as_array().is_none_or(|values| !values.is_empty())) {
        return Err(unsupported("wrapper", "supplement", "$[1]"));
    }
    let object = wrapper
        .as_object()
        .ok_or_else(|| unsupported("wrapper", "object", "$"))?;
    exact_fields(object, "wrapper", &["root", "commentLocations"], "$")?;
    if !object.get("commentLocations").is_some_and(Value::is_array) {
        return Err(unsupported("wrapper", "commentLocations", "$"));
    }
    let block = upstream_node(&object["root"], source, "$.root")?;
    let span = match &block {
        SemanticNode::Block { span, .. } => *span,
        _ => return Err(unsupported("wrapper", "root", "$.root")),
    };
    Ok(SemanticNode::Module {
        span,
        block: Box::new(block),
    })
}

fn ast_payloads(ast: &Value, source: &str) -> Result<AstPayloads, Box<dyn Error>> {
    let mut payloads = Vec::new();
    for object in ast_objects(ast) {
        let Some(name) = object.get("type").and_then(Value::as_str) else {
            continue;
        };
        let keep = matches!(
            name,
            "AstExprConstantNil"
                | "AstExprConstantBool"
                | "AstExprConstantNumber"
                | "AstExprConstantInteger"
                | "AstExprConstantString"
                | "AstExprBinary"
                | "AstExprUnary"
                | "AstTypeReference"
                | "AstTypeSingletonBool"
                | "AstTypeSingletonString"
        );
        if !keep {
            continue;
        }
        let (start, end) = ast_span(object, source)?;
        let payload = match name {
            "AstExprConstantBool" | "AstTypeSingletonBool" => {
                object.get("value").cloned().ok_or("boolean payload")?
            }
            "AstExprConstantNumber" => object.get("bits").cloned().ok_or("number payload")?,
            "AstExprConstantInteger" => object.get("value").cloned().ok_or("integer payload")?,
            "AstExprConstantString" | "AstTypeSingletonString" => {
                object.get("value").cloned().ok_or("string payload")?
            }
            "AstExprBinary" | "AstExprUnary" => {
                object.get("op").cloned().ok_or("operator payload")?
            }
            "AstTypeReference" => object.get("name").cloned().ok_or("type name payload")?,
            _ => Value::Null,
        };
        payloads.push((start, end, name.to_owned(), payload));
    }
    payloads.sort_by_key(|(start, end, name, _)| (*start, *end, name.clone()));
    Ok(payloads)
}

fn instar_span(semantics: &Semantics, node: &instar_core::syntax::SyntaxNode) -> (u32, u32) {
    let relative = if matches!(node.kind(), K::Root | K::Block) {
        node.text_range()
    } else {
        let mut tokens = node
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|token| !token.kind().is_trivia());
        tokens.next().map_or(node.text_range(), |first| {
            text_size::TextRange::new(
                first.text_range().start(),
                tokens.last().unwrap_or(first).text_range().end(),
            )
        })
    };
    let range = semantics.parse().source_range(relative);
    (range.start().into(), range.end().into())
}

fn one_child(
    node: &instar_core::syntax::SyntaxNode,
    kind: K,
    path: &str,
) -> CanonicalResult<instar_core::syntax::SyntaxNode> {
    let mut children = node.children().filter(|child| child.kind() == kind);
    let first = children
        .next()
        .ok_or_else(|| unsupported(&format!("{:?}", node.kind()), &format!("{kind:?}"), path))?;
    if children.next().is_some() {
        return Err(unsupported(
            &format!("{:?}", node.kind()),
            "duplicate child",
            path,
        ));
    }
    Ok(first)
}

fn instar_list(
    semantics: &Semantics,
    container: &instar_core::syntax::SyntaxNode,
    path: &str,
) -> CanonicalResult<Vec<SemanticNode>> {
    container
        .children()
        .enumerate()
        .map(|(index, child)| instar_node(semantics, &child, &format!("{path}[{index}]")))
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "role-preserving syntax projection keeps the supported vocabulary in one dispatch"
)]
fn instar_node(
    semantics: &Semantics,
    node: &instar_core::syntax::SyntaxNode,
    path: &str,
) -> CanonicalResult<SemanticNode> {
    use instar_core::syntax::{NumberValue, number_value, string_bytes};

    let span = instar_span(semantics, node);
    let children = || node.children().collect::<Vec<_>>();
    let binary_parts = || -> CanonicalResult<(String, Box<SemanticNode>, Box<SemanticNode>)> {
        let parts = children();
        if parts.len() != 2 {
            return Err(unsupported("BinaryExpression", "left/right", path));
        }
        let operator = node
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|token| matches!(token.kind(), K::Symbol | K::Keyword))
            .ok_or_else(|| unsupported("BinaryExpression", "operator", path))?;
        Ok((
            operator.text().to_owned(),
            Box::new(instar_node(semantics, &parts[0], &format!("{path}.left"))?),
            Box::new(instar_node(semantics, &parts[1], &format!("{path}.right"))?),
        ))
    };
    match node.kind() {
        K::Root => {
            let block = one_child(node, K::Block, path)?;
            if node.children().count() != 1 {
                return Err(unsupported("Root", "children", path));
            }
            Ok(SemanticNode::Module {
                span,
                block: Box::new(instar_node(semantics, &block, &format!("{path}.block"))?),
            })
        }
        K::Block => Ok(SemanticNode::Block {
            span,
            statements: instar_list(semantics, node, &format!("{path}.statements"))?,
        }),
        // Upstream represents `do ... end` directly as AstStatBlock; Rowan keeps
        // the delimiters in a DoStatement wrapper around its lexical Block.
        K::DoStatement => {
            let block = one_child(node, K::Block, path)?;
            if node.children().count() != 1 {
                return Err(unsupported("DoStatement", "block", path));
            }
            Ok(SemanticNode::Block {
                span,
                statements: instar_list(semantics, &block, &format!("{path}.statements"))?,
            })
        }
        K::ReturnStatement => {
            let lists: Vec<_> = node.children().collect();
            let values = match lists.as_slice() {
                [] => Vec::new(),
                [list] if list.kind() == K::ExpressionList => {
                    instar_list(semantics, list, &format!("{path}.values"))?
                }
                _ => return Err(unsupported("ReturnStatement", "values", path)),
            };
            Ok(SemanticNode::Return { span, values })
        }
        K::LocalStatement => {
            let mut bindings = Vec::new();
            let mut values = Vec::new();
            let mut seen_values = false;
            for part in node.children() {
                match part.kind() {
                    K::Binding if !seen_values => bindings.push(instar_node(
                        semantics,
                        &part,
                        &format!("{path}.bindings[{}]", bindings.len()),
                    )?),
                    K::ExpressionList if !seen_values => {
                        seen_values = true;
                        values = instar_list(semantics, &part, &format!("{path}.values"))?;
                    }
                    kind => return Err(unsupported("LocalStatement", &format!("{kind:?}"), path)),
                }
            }
            Ok(SemanticNode::Local {
                span,
                bindings,
                values,
            })
        }
        K::Binding => {
            let name = one_child(node, K::Name, path)?;
            let token = name
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .find(|token| token.kind() == K::Identifier)
                .ok_or_else(|| unsupported("Binding", "name", path))?;
            let range = semantics.parse().source_range(token.text_range());
            let declaration = semantics
                .declarations()
                .iter()
                .map(|(_, declaration)| declaration)
                .find(|declaration| {
                    declaration.namespace == Namespace::Value && declaration.range == range
                })
                .ok_or_else(|| unsupported("Binding", "declaration", path))?;
            let mut annotation = None;
            for part in node.children().filter(|part| part.kind() != K::Name) {
                if part.kind() != K::TypeAnnotation || annotation.is_some() {
                    return Err(unsupported("Binding", "annotation", path));
                }
                let parts: Vec<_> = part.children().collect();
                let [ty] = parts.as_slice() else {
                    return Err(unsupported("Binding", "annotation type", path));
                };
                annotation = Some(Box::new(instar_node(
                    semantics,
                    ty,
                    &format!("{path}.annotation"),
                )?));
            }
            Ok(SemanticNode::Binding {
                span: (range.start().into(), range.end().into()),
                name: declaration.name.clone(),
                constant: declaration.is_const,
                annotation,
            })
        }
        K::AssignmentStatement => {
            let parts = children();
            let [targets, values] = parts.as_slice() else {
                return Err(unsupported("Assignment", "targets/values", path));
            };
            if targets.kind() != K::ExpressionList || values.kind() != K::ExpressionList {
                return Err(unsupported("Assignment", "expression lists", path));
            }
            let operator = node
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .find(|token| token.kind() == K::Symbol)
                .ok_or_else(|| unsupported("Assignment", "operator", path))?;
            let operator = if operator.text() == "=" {
                None
            } else {
                Some(
                    operator
                        .text()
                        .strip_suffix('=')
                        .ok_or_else(|| unsupported("Assignment", "compound operator", path))?
                        .to_owned(),
                )
            };
            Ok(SemanticNode::Assignment {
                span,
                targets: instar_list(semantics, targets, &format!("{path}.targets"))?,
                values: instar_list(semantics, values, &format!("{path}.values"))?,
                operator,
            })
        }
        K::WhileStatement | K::RepeatStatement => {
            let parts = children();
            let (condition, body) = match parts.as_slice() {
                [condition, body]
                    if node.kind() == K::WhileStatement && body.kind() == K::Block =>
                {
                    (condition, body)
                }
                [body, condition]
                    if node.kind() == K::RepeatStatement && body.kind() == K::Block =>
                {
                    (condition, body)
                }
                _ => return Err(unsupported("Loop", "condition/body", path)),
            };
            let condition = Box::new(instar_node(
                semantics,
                condition,
                &format!("{path}.condition"),
            )?);
            let body = Box::new(instar_node(semantics, body, &format!("{path}.body"))?);
            if node.kind() == K::WhileStatement {
                let has_do = node
                    .children_with_tokens()
                    .filter_map(rowan::NodeOrToken::into_token)
                    .any(|token| token.text() == "do");
                Ok(SemanticNode::While {
                    span,
                    condition,
                    body,
                    has_do,
                })
            } else {
                Ok(SemanticNode::Repeat {
                    span,
                    body,
                    condition,
                })
            }
        }
        K::BreakStatement => Ok(SemanticNode::Break { span }),
        K::ContinueStatement => Ok(SemanticNode::Continue { span }),
        K::VarargExpression => Ok(SemanticNode::Vararg { span }),
        K::ExpressionStatement => {
            let list = one_child(node, K::ExpressionList, path)?;
            let expressions = instar_list(semantics, &list, &format!("{path}.expression"))?;
            let [expression] = expressions
                .try_into()
                .map_err(|_| unsupported("ExpressionStatement", "exactly one expression", path))?;
            Ok(SemanticNode::ExpressionStatement {
                span,
                expression: Box::new(expression),
            })
        }
        K::BinaryExpression => {
            let (operator, left, right) = binary_parts()?;
            Ok(SemanticNode::Binary {
                span,
                operator,
                left,
                right,
            })
        }
        K::UnaryExpression => {
            let parts = children();
            let [operand] = parts.as_slice() else {
                return Err(unsupported("UnaryExpression", "operand", path));
            };
            let operator = node
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .find(|token| matches!(token.kind(), K::Symbol | K::Keyword))
                .ok_or_else(|| unsupported("UnaryExpression", "operator", path))?;
            Ok(SemanticNode::Unary {
                span,
                operator: operator.text().to_owned(),
                operand: Box::new(instar_node(semantics, operand, &format!("{path}.operand"))?),
            })
        }
        K::ParenthesizedExpression => {
            let parts = children();
            let [expression] = parts.as_slice() else {
                return Err(unsupported("ParenthesizedExpression", "expression", path));
            };
            Ok(SemanticNode::Group {
                span,
                expression: Box::new(instar_node(
                    semantics,
                    expression,
                    &format!("{path}.expression"),
                )?),
            })
        }
        K::LiteralExpression => {
            let token = node
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .find(|token| !token.kind().is_trivia())
                .ok_or_else(|| unsupported("LiteralExpression", "token", path))?;
            match token.text() {
                "nil" => Ok(SemanticNode::Nil { span }),
                "true" => Ok(SemanticNode::Bool { span, value: true }),
                "false" => Ok(SemanticNode::Bool { span, value: false }),
                _ if token.kind() == K::Number => match number_value(token.text())
                    .ok_or_else(|| unsupported("LiteralExpression", "number", path))?
                    .value
                {
                    NumberValue::Float(value) => Ok(SemanticNode::Float {
                        span,
                        bits: value.to_bits(),
                    }),
                    NumberValue::Integer(value) => Ok(SemanticNode::Integer { span, bits: value }),
                },
                _ if token.kind() == K::String => Ok(SemanticNode::String {
                    span,
                    bytes: string_bytes(semantics.parse(), &token)
                        .map_err(|_| unsupported("LiteralExpression", "string bytes", path))?,
                }),
                _ => Err(unsupported("LiteralExpression", "literal kind", path)),
            }
        }
        K::NameExpression => {
            let token = node
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .find(|token| token.kind() == K::Identifier)
                .ok_or_else(|| unsupported("NameExpression", "name", path))?;
            let range = semantics.parse().source_range(token.text_range());
            let reference = semantics
                .references()
                .iter()
                .find(|reference| {
                    reference.range == range && reference.namespace == Namespace::Value
                })
                .ok_or_else(|| unsupported("NameExpression", "semantic reference", path))?;
            let local = reference.declaration.and_then(|id| {
                let declaration = &semantics.declarations()[id];
                (!matches!(
                    declaration.kind,
                    DeclarationKind::Global | DeclarationKind::DeclaredFunction
                ))
                .then(|| LocalIdentity {
                    name: declaration.name.clone(),
                    declaration_span: (
                        declaration.range.start().into(),
                        declaration.range.end().into(),
                    ),
                })
            });
            Ok(SemanticNode::Reference {
                span,
                name: reference.name.clone(),
                local,
            })
        }
        K::CallExpression => {
            let parts = children();
            let [callee, arguments] = parts.as_slice() else {
                return Err(unsupported("CallExpression", "callee/arguments", path));
            };
            if arguments.kind() != K::Arguments {
                return Err(unsupported("CallExpression", "arguments", path));
            }
            let argument_nodes = if let Some(list) = arguments
                .children()
                .find(|child| child.kind() == K::ExpressionList)
            {
                if arguments.children().count() != 1 {
                    return Err(unsupported("Arguments", "children", path));
                }
                instar_list(semantics, &list, &format!("{path}.arguments"))?
            } else {
                instar_list(semantics, arguments, &format!("{path}.arguments"))?
            };
            Ok(SemanticNode::Call {
                span,
                callee: Box::new(instar_node(semantics, callee, &format!("{path}.callee"))?),
                arguments: argument_nodes,
            })
        }
        K::FieldExpression => {
            let parts = children();
            let [base, name] = parts.as_slice() else {
                return Err(unsupported("FieldExpression", "base/field", path));
            };
            if name.kind() != K::Name {
                return Err(unsupported("FieldExpression", "field", path));
            }
            let field_name = name
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .find(|token| token.kind() == K::Identifier)
                .ok_or_else(|| unsupported("FieldExpression", "field", path))?;
            Ok(SemanticNode::Field {
                span,
                base: Box::new(instar_node(semantics, base, &format!("{path}.base"))?),
                field: field_name.text().to_owned(),
            })
        }
        K::IndexExpression => {
            let parts = children();
            let [base, index] = parts.as_slice() else {
                return Err(unsupported("IndexExpression", "base/index", path));
            };
            Ok(SemanticNode::Index {
                span,
                base: Box::new(instar_node(semantics, base, &format!("{path}.base"))?),
                index: Box::new(instar_node(semantics, index, &format!("{path}.index"))?),
            })
        }
        kind => Err(unsupported(&format!("{kind:?}"), "node", path)),
    }
}

fn syntax_topology(semantics: &Semantics) -> CanonicalResult<AstTopology> {
    instar_node(semantics, &semantics.parse().syntax(), "$.root")
}

fn compare_topology(expected: &AstTopology, actual: &AstTopology) -> CanonicalResult<()> {
    (expected == actual)
        .then_some(())
        .ok_or_else(|| unsupported("comparison", "semantic mismatch", "$"))
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

fn decoded_string(token: &Value) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    match token.get("decoded").and_then(Value::as_bool) {
        Some(true) => Ok(Some(decode_hex(
            token
                .get("decoded_hex")
                .and_then(Value::as_str)
                .ok_or("decoded string bytes")?,
        )?)),
        Some(false) => Ok(None),
        None => Err("missing string decoding outcome".into()),
    }
}

fn recorded_cases() -> Result<Vec<Value>, Box<dyn Error>> {
    let record = upstream_root()?
        .parent()
        .and_then(Path::parent)
        .ok_or("workspace root")?
        .join(format!(
            "target/oracle/parser-records-{}-{}.jsonl",
            std::process::id(),
            RECORDING.fetch_add(1, Ordering::Relaxed)
        ));
    if record.exists() {
        fs::remove_file(&record)?;
    }
    let output = Command::new(env!("INSTAR_PARSER_TESTS"))
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
    let cases: Vec<Value> = contents
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let mut begun = BTreeSet::new();
    let mut completed = BTreeMap::new();
    let mut lexers = BTreeMap::new();
    for case in &cases {
        match case.get("mode").and_then(Value::as_str) {
            Some("parse_begin") => {
                let id = case["id"].as_u64().ok_or("parser invocation id")?;
                assert!(begun.insert(id), "duplicate parser entry {id}");
            }
            Some("parse") => {
                let id = case["id"].as_u64().ok_or("parser result id")?;
                assert!(
                    completed.insert(id, case).is_none(),
                    "duplicate parser result {id}"
                );
            }
            Some("parser_lex") => {
                let id = case["parent"].as_u64().ok_or("parser lexer id")?;
                assert!(
                    lexers.insert(id, case).is_none(),
                    "multiple lexers for parser {id}"
                );
            }
            Some("lex") => assert_eq!(
                case["start"],
                serde_json::json!([0, 0]),
                "unhandled standalone lexer origin"
            ),
            mode => return Err(format!("unknown recording kind {mode:?}").into()),
        }
    }
    assert_eq!(
        begun,
        completed.keys().copied().collect(),
        "unrecorded parser exits"
    );
    assert_eq!(
        begun,
        lexers.keys().copied().collect(),
        "unrecorded parser lexers"
    );
    for (id, parse) in completed {
        assert_eq!(
            parse["source_hex"], lexers[&id]["source_hex"],
            "parser/lexer input mismatch {id}"
        );
    }
    Ok(cases)
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
    let limits = case
        .get("limits")
        .and_then(Value::as_object)
        .ok_or("recorded limits")?;
    let optional_limit = |name: &str, default: u64| -> Result<Option<usize>, Box<dyn Error>> {
        let value = limits
            .get(name)
            .and_then(Value::as_u64)
            .ok_or("recorded limit value")?;
        Ok((value != default).then_some(usize::try_from(value)?))
    };
    Ok(ParseOptions {
        features,
        recursion_limit: optional_limit("LuauRecursionLimit", 1000)?,
        type_length_limit: optional_limit("LuauTypeLengthLimit", 1000)?,
        error_limit: optional_limit("LuauParseErrorLimit", 100)?,
    })
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
        // `recorded_options` replays every recorded limit, including nondefaults.
        let bytes = decode_hex(
            case.get("source_hex")
                .and_then(Value::as_str)
                .ok_or("recorded source")?,
        )?;
        let upstream_ok = case
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        let entry = match case.get("entry").and_then(Value::as_str) {
            Some("module") => EntryPoint::Module,
            Some("expression") => EntryPoint::Expression,
            Some("type") => EntryPoint::Type,
            other => return Err(format!("unhandled parser entry {other:?}").into()),
        };
        let instar = instar_parse_with(&bytes, recorded_options(case)?, entry)?;
        let view = instar.syntax().to_string();
        let input = view.as_str();
        let syntax_errors = format!("{:?}", instar.errors());
        let syntax = instar.syntax().clone();
        let semantics = Semantics::new(instar);
        if upstream_ok != semantics.is_complete() {
            differences.push(format!(
                "#{index}: upstream={upstream_ok}, syntax={syntax_errors}, semantics={:?}, limits={:?}, source={input:?}",
                semantics.errors(),
                case.get("limits")
            ));
        } else if upstream_ok {
            let complete_ast = format!(
                "[{},{}]",
                case.get("ast")
                    .and_then(Value::as_str)
                    .ok_or("recorded AST")?,
                case.get("ast_supplement")
                    .and_then(Value::as_str)
                    .ok_or("recorded AST supplement")?
            );
            let ast_value: Value = serde_json::from_str(&complete_ast)
                .map_err(|error| format!("case #{index}: AST decoding: {error}; {complete_ast}"))?;
            let ast = &ast_value;
            if !number_values_match(ast, semantics.parse())? {
                ast_differences.push(format!(
                    "#{index}: numeric value/location mismatch, source={input:?}"
                ));
            }
            let semantic_payloads = ast_payloads(ast, input)?;
            let expected_atoms: Vec<_> = semantic_payloads
                .iter()
                .filter_map(|(start, end, name, value)| {
                    if matches!(name.as_str(), "AstExprConstantNil" | "AstExprConstantBool") {
                        Some((*start, *end, value.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            let actual_atoms: Vec<_> = syntax
                .descendants()
                .filter(|node| node.kind() == K::LiteralExpression)
                .filter_map(|node| {
                    let token = node
                        .children_with_tokens()
                        .filter_map(rowan::NodeOrToken::into_token)
                        .find(|token| !token.kind().is_trivia())?;
                    let value = match token.text() {
                        "nil" => Value::Null,
                        "true" => Value::Bool(true),
                        "false" => Value::Bool(false),
                        _ => return None,
                    };
                    Some((
                        u32::from(node.text_range().start()),
                        u32::from(node.text_range().end()),
                        value,
                    ))
                })
                .collect();
            if expected_atoms != actual_atoms {
                ast_differences.push(format!("#{index}: boolean/nil payload mismatch: upstream={expected_atoms:?}, instar={actual_atoms:?}, source={input:?}"));
            }
            match (ast_topology(ast, input), syntax_topology(&semantics)) {
                (Ok(expected), Ok(actual)) if compare_topology(&expected, &actual).is_ok() => {}
                (expected, actual) => ast_differences.push(format!(
                    "#{index} canonical AST: upstream={expected:?}, instar={actual:?}, source={input:?}"
                )),
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
                .flat_map(|node| node.children_with_tokens())
                .filter_map(rowan::NodeOrToken::into_token)
                .filter(|token| matches!(token.text(), "if" | "elseif"))
                .count();
            if upstream_if_expressions != instar_if_expressions {
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
                .filter(|node| node.kind() == K::IfBranch)
                .filter(|node| {
                    node.children_with_tokens()
                        .filter_map(rowan::NodeOrToken::into_token)
                        .any(|token| matches!(token.text(), "if" | "elseif"))
                })
                .count();
            if upstream_if_statements != instar_if_statements {
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
                                | DeclarationKind::Class
                        )
                })
                .map(|(_, declaration)| declaration.name.as_str())
                .collect();
            instar_bindings.sort_unstable();
            if upstream_bindings != instar_bindings {
                ast_differences.push(format!(
                    "#{index} bindings: upstream={upstream_bindings:?}, instar={instar_bindings:?}, source={input:?}"
                ));
            }
        } else {
            let upstream_ranges =
                oracle_item_ranges(case, "errors", input).ok_or("recorded diagnostic locations")?;
            let expected: Vec<_> = case["errors"]
                .as_array()
                .ok_or("diagnostics")?
                .iter()
                .zip(upstream_ranges)
                .map(|(error, range)| {
                    Ok((
                        range,
                        error["message"].as_str().ok_or("diagnostic message")?,
                    ))
                })
                .collect::<Result<_, Box<dyn Error>>>()?;
            let actual: Vec<_> = semantics
                .parse()
                .errors()
                .iter()
                .map(|error| {
                    (
                        (u32::from(error.range.start()), u32::from(error.range.end())),
                        error.message.as_str(),
                    )
                })
                .chain(semantics.errors().iter().map(|error| {
                    (
                        (u32::from(error.range.start()), u32::from(error.range.end())),
                        error.message.as_str(),
                    )
                }))
                .collect();
            if expected != actual {
                diagnostic_differences.push(format!(
                    "#{index}: upstream={expected:?}, instar={actual:?}, source={input:?}"
                ));
            }
        }
    }
    let acceptance_summary = differences.join("\n");
    let ast_summary = ast_differences.join("\n");
    assert!(
        acceptance_summary.is_empty()
            && ast_summary.is_empty()
            && diagnostic_differences.is_empty(),
        "acceptance={} AST={} diagnostics={} across {} invocations\n{}\n{}\n{}",
        acceptance_summary.lines().count(),
        ast_summary.lines().count(),
        diagnostic_differences.len(),
        cases.len(),
        acceptance_summary,
        ast_summary,
        diagnostic_differences.join("\n")
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
    for path in &paths {
        let bytes = fs::read(path)?;
        let upstream = oracle(&["parse", "all"], &bytes)?;
        let upstream_ok = upstream
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        let instar = instar_parse(&bytes)?;
        if upstream_ok != instar.errors().is_empty() {
            differences.push(format!(
                "{}: upstream={upstream_ok}, instar={:?}",
                path.display(),
                instar.errors()
            ));
        }
    }

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

fn oracle_token_kinds(kind: u64, spelling: &str) -> Option<Vec<K>> {
    let single = match kind {
        1..=255 => {
            if b"+-*/%^#=<>~(){}[];:,.?|&@!".contains(&u8::try_from(kind).ok()?) {
                K::Symbol
            } else {
                K::Invalid
            }
        }
        257..=265 | 270..=277 => K::Symbol,
        269 | 278 | 279 => K::String,
        280 => {
            if instar_core::syntax::number_value(spelling).is_some() {
                K::Number
            } else {
                K::Invalid
            }
        }
        281 => K::Identifier,
        282 | 283 | 287 => K::Comment,
        284 => return Some(vec![K::Symbol, K::Identifier]),
        285 => return Some(vec![K::Symbol, K::Symbol]),
        288 | 290 => K::Invalid,
        286 if !spelling.starts_with(['`', '}']) => K::Invalid,
        266..=268 | 286 | 289 => {
            let mut kinds = Vec::new();
            if spelling.starts_with('`') {
                kinds.push(K::InterpolationStart);
            } else if spelling.starts_with('}') {
                kinds.push(K::InterpolationClose);
            }
            // Broken interpolation lexemes stop before the failed delimiter.
            let closed = matches!(kind, 266..=268);
            if spelling.len() > 1 + usize::from(closed) {
                kinds.push(K::InterpolationText);
            }
            if closed {
                kinds.push(if kind == 268 {
                    K::InterpolationEnd
                } else {
                    K::InterpolationOpen
                });
            }
            return Some(kinds);
        }
        291..=311 => K::Keyword,
        _ => return None,
    };
    Some(vec![single])
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
        let instar = instar_parse(&bytes)?;
        let view = instar.syntax().to_string();
        let input = view.as_str();
        let profile = if case.get("integer").and_then(Value::as_bool) == Some(true) {
            "all"
        } else {
            "default"
        };
        let upstream = oracle(&["lex", profile], &bytes)?;
        let upstream_ranges =
            oracle_ranges(&upstream, input).ok_or("invalid lexer oracle response")?;
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
                if token.get("type").and_then(Value::as_u64).is_some_and(|kind| matches!(kind, 286..=290))
                    && !instar.errors().iter().any(|error| {
                        u32::from(error.range.start()) <= range.1
                            && range.0 <= u32::from(error.range.end())
                    }) {
                    differences.push(format!("#{index} broken upstream token without an Instar diagnostic at {range:?}"));
                    return false;
                }
                let expected = token.get("type").and_then(Value::as_u64)
                    .and_then(|kind| oracle_token_kinds(kind, &input[range.0 as usize..range.1 as usize]));
                let actual: Vec<_> = tokens.iter().filter(|candidate| {
                    u32::from(candidate.text_range().start()) >= range.0
                        && u32::from(candidate.text_range().end()) <= range.1
                }).map(rowan::SyntaxToken::kind).collect();
                if expected.as_ref() == Some(&actual) {
                    true
                } else {
                    differences.push(format!("#{index} token category {:?} at {range:?}: expected={expected:?}, actual={actual:?}", token.get("type")));
                    false
                }
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
            .map(decoded_string)
            .collect();
        let instar_strings: Vec<_> = tokens
            .iter()
            .filter(|token| token.kind() == K::String)
            .map(|token| instar_core::syntax::string_bytes(&instar, token).ok())
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
fn oracle_comparisons_fail_closed() -> TestResult {
    assert_eq!(oracle_token_kinds(256, ""), None);
    assert_eq!(oracle_token_kinds(312, ""), None);
    assert!(decoded_string(&serde_json::json!({})).is_err());
    assert!(decoded_string(&serde_json::json!({"decoded": true})).is_err());
    assert_eq!(
        decoded_string(&serde_json::json!({"decoded": false}))?,
        None
    );
    assert_eq!(
        decoded_string(&serde_json::json!({"decoded": true, "decoded_hex": ""}))?,
        Some(Vec::new())
    );
    let parsed = instar_parse("return '\\xGG'")?;
    let token = parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|token| token.kind() == K::String)
        .ok_or("string")?;
    let upstream = oracle(&["lex", "all"], "return '\\xGG'")?;
    let upstream = upstream["tokens"]
        .as_array()
        .ok_or("tokens")?
        .iter()
        .find(|token| token["type"].as_u64() == Some(279))
        .ok_or("upstream string")?;
    assert_eq!(
        instar_core::syntax::string_bytes(&parsed, &token).ok(),
        decoded_string(upstream)?
    );
    Ok(())
}

fn canonical_pair(source: &str) -> Result<(AstTopology, AstTopology), Box<dyn Error>> {
    let response = oracle(&["parse", "all"], source)?;
    assert_eq!(
        response["errors"]
            .as_array()
            .ok_or("oracle errors")?
            .as_slice(),
        &[] as &[Value]
    );
    let encoded = response["ast"].as_str().ok_or("oracle AST")?;
    let upstream: Value = serde_json::from_str(encoded)?;
    let expected = ast_topology(&upstream, source)?;
    let semantics = Semantics::new(instar_parse(source)?);
    assert!(semantics.is_complete(), "{:?}", semantics.errors());
    let actual = syntax_topology(&semantics)?;
    Ok((expected, actual))
}

#[test]
fn canonical_semantic_slice_matches_independent_parsers() -> TestResult {
    for source in [
        "return a + b * 2",
        "local a, b = 1, 2; a, b = b, a; a += b; return a",
        "const a = 1; local b = a; return b",
        "local a = 0; while a < 2 do a += 1 end; repeat a -= 1 until a == 0; return a",
        "while true do break end; while false do continue end",
        "f(a, x.y[z])",
        "do f() end",
        "do foo 'bar' end",
        "return -((nil)), true, false, 2.5, 1i, 'a\\0b'",
    ] {
        let (expected, actual) = canonical_pair(source)?;
        assert_eq!(expected, actual, "{source}");
    }
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "all mutations are checked against one untouched independently projected tree"
)]
fn canonical_comparison_detects_role_payload_and_parent_mutations() -> TestResult {
    let (_, actual) = canonical_pair("return (a + b) * 2.5, f(x.y[z], 'hi')")?;

    let mut changed = actual.clone();
    let SemanticNode::Module { block, .. } = &mut changed else {
        unreachable!()
    };
    let SemanticNode::Block { statements, .. } = block.as_mut() else {
        unreachable!()
    };
    let SemanticNode::Return { values, .. } = &mut statements[0] else {
        unreachable!()
    };
    let SemanticNode::Binary { left, right, .. } = &mut values[0] else {
        unreachable!()
    };
    std::mem::swap(left, right);
    assert!(
        compare_topology(&changed, &actual).is_err(),
        "left/right roles collapsed"
    );

    let mut changed = actual.clone();
    let SemanticNode::Module { block, .. } = &mut changed else {
        unreachable!()
    };
    let SemanticNode::Block { statements, .. } = block.as_mut() else {
        unreachable!()
    };
    let SemanticNode::Return { values, .. } = &mut statements[0] else {
        unreachable!()
    };
    let SemanticNode::Binary { left, .. } = &mut values[0] else {
        unreachable!()
    };
    let SemanticNode::Group { span, .. } = left.as_mut() else {
        unreachable!()
    };
    span.0 += 1;
    assert!(
        compare_topology(&changed, &actual).is_err(),
        "nested parent discarded"
    );

    let mut changed = actual.clone();
    let SemanticNode::Module { block, .. } = &mut changed else {
        unreachable!()
    };
    let SemanticNode::Block { statements, .. } = block.as_mut() else {
        unreachable!()
    };
    let SemanticNode::Return { values, .. } = &mut statements[0] else {
        unreachable!()
    };
    let SemanticNode::Binary { operator, .. } = &mut values[0] else {
        unreachable!()
    };
    *operator = "/".into();
    assert!(
        compare_topology(&changed, &actual).is_err(),
        "operator discarded"
    );

    let mut changed = actual.clone();
    let SemanticNode::Module { block, .. } = &mut changed else {
        unreachable!()
    };
    let SemanticNode::Block { statements, .. } = block.as_mut() else {
        unreachable!()
    };
    let SemanticNode::Return { values, .. } = &mut statements[0] else {
        unreachable!()
    };
    let SemanticNode::Binary { right, .. } = &mut values[0] else {
        unreachable!()
    };
    let SemanticNode::Float { bits, .. } = right.as_mut() else {
        unreachable!()
    };
    *bits ^= 1;
    assert!(
        compare_topology(&changed, &actual).is_err(),
        "numeric bits discarded"
    );

    let mut changed = actual.clone();
    let SemanticNode::Module { block, .. } = &mut changed else {
        unreachable!()
    };
    let SemanticNode::Block { statements, .. } = block.as_mut() else {
        unreachable!()
    };
    let SemanticNode::Return { values, .. } = &mut statements[0] else {
        unreachable!()
    };
    let SemanticNode::Call { arguments, .. } = &mut values[1] else {
        unreachable!()
    };
    arguments.swap(0, 1);
    assert!(
        compare_topology(&changed, &actual).is_err(),
        "argument order discarded"
    );

    let mut changed = actual.clone();
    let SemanticNode::Module { block, .. } = &mut changed else {
        unreachable!()
    };
    let SemanticNode::Block { statements, .. } = block.as_mut() else {
        unreachable!()
    };
    let SemanticNode::Return { values, .. } = &mut statements[0] else {
        unreachable!()
    };
    let SemanticNode::Call { arguments, .. } = &mut values[1] else {
        unreachable!()
    };
    let SemanticNode::String { bytes, .. } = &mut arguments[1] else {
        unreachable!()
    };
    bytes[0] ^= 1;
    assert!(
        compare_topology(&changed, &actual).is_err(),
        "string bytes discarded"
    );
    Ok(())
}

#[test]
fn canonical_references_use_semantic_local_identity() -> TestResult {
    let source = "local a = 1; return a";
    let response = oracle(&["parse", "all"], source)?;
    let wrapper: Value = serde_json::from_str(response["ast"].as_str().ok_or("AST")?)?;
    let upstream_local = ast_objects(&wrapper)
        .into_iter()
        .find(|object| object.get("type").and_then(Value::as_str) == Some("AstExprLocal"))
        .ok_or("upstream local reference")?;
    let expected = upstream_node(&Value::Object(upstream_local.clone()), source, "$.local")?;

    let semantics = Semantics::new(instar_parse(source)?);
    let name = semantics
        .parse()
        .syntax()
        .descendants()
        .find(|node| node.kind() == K::NameExpression)
        .ok_or("instar local reference")?;
    let actual = instar_node(&semantics, &name, "$.local")?;
    assert_eq!(expected, actual);

    let mut changed_name = actual.clone();
    let SemanticNode::Reference { name, .. } = &mut changed_name else {
        unreachable!()
    };
    *name = "b".into();
    assert!(compare_topology(&changed_name, &actual).is_err());

    let mut changed_identity = actual.clone();
    let SemanticNode::Reference { local, .. } = &mut changed_identity else {
        unreachable!()
    };
    local.as_mut().ok_or("local identity")?.declaration_span.0 += 1;
    assert!(compare_topology(&changed_identity, &actual).is_err());
    Ok(())
}

#[test]
fn canonical_projection_fails_closed() {
    let unknown_node = serde_json::json!({
        "root": {"type": "AstStatMystery", "location": "0,0 - 0,0"},
        "commentLocations": []
    });
    assert!(
        ast_topology(&unknown_node, "")
            .unwrap_err()
            .0
            .contains("AstStatMystery")
    );

    let unknown_field = serde_json::json!({
        "root": {"type": "AstStatBlock", "location": "0,0 - 0,0", "hasEnd": true,
                 "body": [], "newSemanticMeaning": true},
        "commentLocations": []
    });
    assert!(
        ast_topology(&unknown_field, "")
            .unwrap_err()
            .0
            .contains("newSemanticMeaning")
    );
}

#[test]
fn original_byte_literals_match_upstream() -> TestResult {
    for input in [
        b"return '\xff', [[\xfe]], `\xfd`".as_slice(),
        b"--\xff\nreturn '\xe2\x82'".as_slice(),
        b"local \xff = 1".as_slice(),
    ] {
        let upstream = oracle(&["parse", "all"], input)?;
        let parsed = instar_parse(input)?;
        assert_eq!(parsed.source().bytes(), input);
        assert_eq!(
            parsed.errors().is_empty(),
            upstream["errors"].as_array().ok_or("errors")?.is_empty()
        );
        let upstream = oracle(&["lex", "all"], input)?;
        let expected: Result<Vec<_>, _> = upstream["tokens"]
            .as_array()
            .ok_or("tokens")?
            .iter()
            .filter(|token| matches!(token["type"].as_u64(), Some(269 | 278 | 279)))
            .map(decoded_string)
            .collect();
        let actual: Result<Vec<_>, _> = parsed
            .syntax()
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|token| token.kind() == K::String)
            .map(|token| instar_core::syntax::string_bytes(&parsed, &token))
            .collect();
        assert_eq!(actual?.into_iter().map(Some).collect::<Vec<_>>(), expected?);
    }
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
            let request = instar_core::syntax::string_bytes(semantics.parse(), &token)?;
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
