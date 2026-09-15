use super::{
    Edit, Finding,
    configuration::{Level, Settings},
    registry,
};

use crate::source::{PositionEncoding, Source};
use serde_json::Value;

pub(super) use crate::syntax::{array, field, kind};

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

pub(super) fn unwrap(mut value: &Value) -> &Value {
    while matches!(
        kind(value),
        "AstExprGroup" | "AstExprTypeAssertion" | "AstExprInstantiate"
    ) {
        value = &value["expr"];
    }

    value
}

pub(super) fn number(value: &Value) -> Option<f64> {
    let value = unwrap(value);

    if kind(value) == "AstExprConstantNumber" {
        value["value"]
            .as_f64()
            .or_else(|| value["value"].as_str()?.parse().ok())
    } else if kind(value) == "AstExprUnary" && field(value, "op") == "Minus" {
        number(&value["expr"]).map(|value| -value)
    } else {
        None
    }
}

pub(super) fn truth(value: &Value) -> Option<bool> {
    let value = unwrap(value);

    match kind(value) {
        "AstExprConstantNil" => Some(false),
        "AstExprConstantBool" => value["value"].as_bool(),

        "AstExprConstantNumber" | "AstExprConstantString" | "AstExprTable" | "AstExprFunction" => {
            Some(true)
        }

        _ => None,
    }
}

pub(super) fn global(value: &Value) -> Option<String> {
    let value = unwrap(value);

    match kind(value) {
        "AstExprGlobal" => Some(field(value, "global").into()),

        "AstExprIndexName" if field(value, "op") == "." => Some(format!(
            "{}.{}",
            global(&value["expr"])?,
            field(value, "index")
        )),

        _ => None,
    }
}

pub(super) fn local(value: &Value) -> Option<&str> {
    (kind(value) == "AstExprLocal").then(|| field(&value["local"], "location"))
}

pub(super) fn comparison(value: &Value) -> bool {
    kind(value) == "AstExprBinary" && field(value, "op").starts_with("Compare")
}

pub(super) fn call(value: &Value, name: &str) -> bool {
    kind(value) == "AstExprCall" && global(&value["func"]).as_deref() == Some(name)
}

pub(super) fn expands(value: &Value) -> bool {
    matches!(kind(value), "AstExprCall" | "AstExprVarargs")
}

pub(super) struct Node<'value> {
    pub value: &'value Value,
    pub parent: &'value Value,
    pub scope: &'value Value,
}

fn collect<'value>(
    value: &'value Value,
    parent: &'value Value,
    scope: &'value Value,
    nodes: &mut Vec<Node<'value>>,
) {
    match value {
        Value::Object(fields) => {
            if !kind(value).is_empty() {
                nodes.push(Node {
                    value,
                    parent,
                    scope,
                });
            }

            let scope = if matches!(
                kind(value),
                "AstStatBlock" | "AstExprFunction" | "AstStatFor" | "AstStatForIn"
            ) {
                value
            } else {
                scope
            };

            for (name, child) in fields {
                if (kind(value) == "AstExprLocal" && name == "local")
                    || (kind(value) == "AstTypeReference" && name == "prefixLocal")
                {
                    continue;
                }

                collect(child, value, scope, nodes);
            }
        }

        Value::Array(values) => {
            for child in values {
                collect(child, parent, scope, nodes);
            }
        }

        _ => {}
    }
}

pub(super) struct Context<'value> {
    pub source: &'value Source,
    pub settings: &'value Settings,
    pub nodes: Vec<Node<'value>>,
    pub globals: BTreeSet<String>,
    pub roblox: bool,
    pub reads: BTreeMap<String, Vec<&'value Value>>,
    pub writes: BTreeSet<String>,
    pub mutated: BTreeSet<String>,
    pub findings: Vec<Finding>,
    pub comments: Vec<Range<usize>>,
    suppressions: BTreeMap<u32, BTreeSet<String>>,
    file_suppressions: BTreeSet<String>,
}

impl<'value> Context<'value> {
    pub(super) fn new(
        source: &'value Source,
        document: &'value Value,
        settings: &'value Settings,
        globals: BTreeSet<String>,
        roblox: bool,
    ) -> Self {
        let root = &document["root"];
        let mut nodes = Vec::new();
        collect(root, root, root, &mut nodes);
        let mut writes = BTreeSet::new();
        let mut written = BTreeSet::new();
        let mut mutated = BTreeSet::new();

        for node in &nodes {
            let value = node.value;

            let targets = match kind(value) {
                "AstStatAssign" => array(&value["vars"]).iter().collect::<Vec<_>>(),
                "AstStatCompoundAssign" => vec![&value["var"]],
                _ => Vec::new(),
            };

            for target in targets {
                if let Some(binding) = local(target) {
                    writes.insert(binding.into());

                    if kind(value) != "AstStatCompoundAssign" {
                        written.insert(field(target, "location").to_owned());
                    }
                }

                let mut base = target;

                while matches!(kind(base), "AstExprIndexName" | "AstExprIndexExpr") {
                    base = &base["expr"];
                }

                if let Some(binding) = local(base) {
                    mutated.insert(binding.into());
                }
            }
        }

        let mut reads = BTreeMap::<String, Vec<&Value>>::new();

        for node in &nodes {
            let binding = if kind(node.value) == "AstTypeReference" {
                node.value["prefixLocal"]["location"].as_str()
            } else {
                local(node.value)
            };

            if let Some(binding) = binding
                && !written.contains(field(node.value, "location"))
            {
                reads.entry(binding.into()).or_default().push(node.value);
            }
        }

        let mut context = Self {
            source,
            settings,
            nodes,
            globals,
            roblox,
            reads,
            writes,
            mutated,
            findings: Vec::new(),
            comments: Vec::new(),
            suppressions: BTreeMap::new(),
            file_suppressions: BTreeSet::new(),
        };

        for comment in array(&document["commentLocations"]) {
            if let Some(range) = context.span(comment) {
                context.comments.push(range);
            }
        }

        context.suppressions();

        context
    }

    pub(super) fn span(&self, value: &Value) -> Option<Range<usize>> {
        self.location(field(value, "location"))
    }

    pub(super) fn location(&self, location: &str) -> Option<Range<usize>> {
        let (start, end) = location.split_once(" - ")?;

        let offset = |position: &str| {
            let (line, col) = position.split_once(',')?;

            self.source
                .offset(
                    line_index::LineCol {
                        line: line.parse().ok()?,
                        col: col.parse().ok()?,
                    },
                    PositionEncoding::Utf8,
                )
                .ok()
                .map(usize::from)
        };

        let range = offset(start)?..offset(end)?;

        (range.start <= range.end && self.source.bytes().get(range.clone()).is_some())
            .then_some(range)
    }

    pub(super) fn text(&self, value: &Value) -> &str {
        self.span(value)
            .and_then(|range| self.source.text().ok()?.get(range))
            .unwrap_or("")
    }

    pub(super) fn same(&self, left: &Value, right: &Value) -> bool {
        if kind(left) != kind(right) {
            return false;
        }

        if let (Some(left), Some(right)) = (local(left), local(right)) {
            return left == right;
        }

        let left = self.text(left);
        let right = self.text(right);

        !left.is_empty() && left == right
    }

    pub(super) fn emit(&mut self, rule: &str, node: &Value, message: impl Into<String>) {
        if let Some(range) = self.span(node) {
            self.emit_range(rule, range, message, Vec::new());
        }
    }

    pub(super) fn emit_range(
        &mut self,
        rule: &str,
        range: Range<usize>,
        message: impl Into<String>,
        edits: Vec<Edit>,
    ) {
        let level = self.settings.level(rule);

        if level == Level::Allow
            || (registry::find(rule).is_some_and(|rule| rule.group == "roblox") && !self.roblox)
        {
            return;
        }

        let line = self
            .source
            .position(
                line_index::TextSize::try_from(range.start).unwrap_or_default(),
                PositionEncoding::Utf8,
            )
            .map(|position| position.line)
            .unwrap_or_default();

        if self.file_suppressions.contains(rule)
            || self.file_suppressions.contains("*")
            || self
                .suppressions
                .get(&line)
                .is_some_and(|rules| rules.contains(rule) || rules.contains("*"))
        {
            return;
        }

        self.findings.push(Finding {
            rule: rule.into(),
            level,
            message: message.into(),
            start: range.start,
            end: range.end,
            edits,
        });
    }

    pub(super) fn fix(&mut self, rule: &str, node: &Value, replacement: String) {
        if let Some(range) = self.span(node) {
            let edits = if self
                .comments
                .iter()
                .any(|comment| comment.start < range.end && range.start < comment.end)
            {
                Vec::new()
            } else {
                vec![Edit {
                    start: range.start,
                    end: range.end,
                    text: replacement,
                }]
            };

            self.emit_range(
                rule,
                range,
                registry::find(rule).expect("registered rule").description,
                edits,
            );
        }
    }

    fn suppressions(&mut self) {
        for range in self.comments.clone() {
            let text = self
                .source
                .text()
                .ok()
                .and_then(|text| text.get(range.clone()))
                .unwrap_or("");

            if let Some(names) = text.strip_prefix("--!nolint") {
                if names.trim().is_empty() {
                    self.file_suppressions.insert("*".into());
                } else {
                    for name in names.split_whitespace() {
                        if let Some(rule) = registry::upstream(name) {
                            self.file_suppressions.insert(rule.into());
                        }
                    }
                }

                continue;
            }

            let Some(directive) = text
                .strip_prefix("--")
                .map(str::trim_start)
                .and_then(|text| text.strip_prefix("instar:"))
                .map(str::trim)
            else {
                continue;
            };

            let Some(names) = directive
                .strip_prefix("allow(")
                .and_then(|text| text.strip_suffix(')'))
            else {
                self.emit_range(
                    "invalid_directive",
                    range,
                    "Expected -- instar: allow(rule, ...)",
                    Vec::new(),
                );

                continue;
            };

            let names = names.split(',').map(str::trim).collect::<Vec<_>>();

            if names
                .iter()
                .any(|name| *name != "*" && registry::find(name).is_none() && !name.contains('/'))
            {
                self.emit_range(
                    "invalid_directive",
                    range,
                    "Unknown rule in suppression",
                    Vec::new(),
                );

                continue;
            }

            if let Ok(position) = self.source.position(
                line_index::TextSize::try_from(range.start).unwrap_or_default(),
                PositionEncoding::Utf8,
            ) {
                for line in [position.line, position.line + 1] {
                    self.suppressions
                        .entry(line)
                        .or_default()
                        .extend(names.iter().map(|name| (*name).into()));
                }
            }
        }
    }
}

pub(super) fn decode(text: &str) -> Result<Value, serde_json::Error> {
    let mut output = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut index = 0;

    while index < text.len() {
        let tail = &text[index..];

        if !quoted
            && let Some(number) = ["-Infinity", "Infinity", "NaN"]
                .into_iter()
                .find(|number| tail.starts_with(number))
        {
            output.push('"');
            output.push_str(number);
            output.push('"');
            index += number.len();
            continue;
        }

        let character = tail.chars().next().expect("remaining character");
        output.push(character);

        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        }

        index += character.len_utf8();
    }

    serde_json::from_str(&output)
}
