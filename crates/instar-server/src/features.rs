use std::collections::BTreeSet;

use serde_json::{Value, json};
use vermis::{Kind, Parts, Span, TokenKind, Tree, View};

use crate::document::Document;

pub(crate) struct Dependency {
    pub(crate) name: String,
    pub(crate) argument: Span,
    pub(crate) service: Option<String>,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) struct Local {
    pub(crate) name: Span,
    pub(crate) statement: Span,
    pub(crate) replacement: Option<String>,
}

pub(crate) struct Syntax {
    pub(crate) names: BTreeSet<String>,
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) exports: BTreeSet<String>,
    pub(crate) locals: Vec<Local>,
    pub(crate) require_bindings: Vec<Span>,
    callees: Vec<(Span, Span)>,
    comments: Vec<(Span, String)>,
    colors: Vec<(Span, [f64; 3], String)>,
    pub(crate) insertion: usize,
}

fn text(node: View<'_, '_>) -> String {
    String::from_utf8_lossy(node.text()).into_owned()
}

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();

    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !matches!(
            value,
            "and"
                | "break"
                | "do"
                | "else"
                | "elseif"
                | "end"
                | "false"
                | "for"
                | "function"
                | "if"
                | "in"
                | "local"
                | "nil"
                | "not"
                | "or"
                | "repeat"
                | "return"
                | "then"
                | "true"
                | "until"
                | "while"
        )
}

pub(crate) fn binding_name(value: &str) -> Option<String> {
    identifier(value).then(|| value.to_owned())
}

fn top_level<'tree, 'source>(tree: &'tree Tree<'source>) -> Vec<View<'tree, 'source>> {
    let Some(Parts::Root { block }) = tree.view(tree.root).and_then(View::parts) else {
        return Vec::new();
    };

    block.children().collect()
}

impl Syntax {
    pub(crate) fn new(tree: &Tree<'_>) -> Self {
        let mut result = Self {
            names: BTreeSet::new(),
            dependencies: Vec::new(),
            exports: BTreeSet::new(),
            locals: Vec::new(),
            require_bindings: Vec::new(),
            callees: Vec::new(),
            comments: Vec::new(),
            colors: Vec::new(),
            insertion: 0,
        };

        result.comments = comments(tree);

        let statements = top_level(tree);

        result.insertion = statements
            .first()
            .map_or(tree.source.len(), |node| node.span().start);

        for node in tree
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, _)| tree.view(index))
        {
            match node.parts() {
                Some(Parts::Binding { name, .. }) => {
                    result.names.insert(text(name));
                }

                Some(Parts::Function {
                    name: Some(name), ..
                }) if name.kind() == Kind::Name => {
                    result.names.insert(text(name));
                }

                Some(Parts::Call { callee, .. }) => {
                    result.callees.push((node.span(), callee.span()));
                }

                Some(Parts::MethodCall { method, .. }) => {
                    result.callees.push((node.span(), method.span()));
                }

                Some(Parts::Assignment { targets, .. }) => {
                    for target in targets.filter(|target| target.kind() == Kind::Name) {
                        result.names.insert(text(target));
                    }
                }

                _ => {}
            }

            if let Some(Parts::Local { bindings, values }) = node.parts() {
                let bindings: Vec<_> = bindings.collect();
                let values: Vec<_> = values.collect();

                for (binding, value) in bindings.iter().zip(&values) {
                    if let Some(Parts::Binding { name, .. }) = binding.parts()
                        && matches!(value.parts(), Some(Parts::Call { callee, .. }) if callee.text() == b"require")
                    {
                        result.require_bindings.push(name.span());
                    }
                }

                if bindings.len() == 1
                    && values.len() <= 1
                    && let Some(Parts::Binding { name, .. }) = bindings[0].parts()
                {
                    let replacement =
                        values
                            .first()
                            .map_or(Some(String::new()), |value| match value.kind() {
                                Kind::Call | Kind::MethodCall => Some(text(*value)),
                                Kind::Number | Kind::String | Kind::Function => Some(String::new()),

                                _ if matches!(value.text(), b"nil" | b"true" | b"false") => {
                                    Some(String::new())
                                }

                                _ => None,
                            });

                    result.locals.push(Local {
                        name: name.span(),
                        statement: node.span(),
                        replacement,
                    });
                }
            }
        }

        result.dependencies = dependencies(&statements);
        result.exports = exports(&statements);

        if !result.names.contains("Color3") {
            for node in tree
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(index, _)| tree.view(index))
            {
                if let Some(color) = color_call(node) {
                    result.colors.push((node.span(), color.0, color.1));
                }
            }
        }

        result
    }

    pub(crate) fn documentation(&self, document: &Document, offset: usize) -> Option<String> {
        let mut start = document.text[..offset.min(document.text.len())]
            .rfind('\n')
            .map_or(0, |index| index + 1);

        let mut parts = Vec::new();

        for (span, body) in self
            .comments
            .iter()
            .rev()
            .filter(|(span, _)| span.end <= start)
            .collect::<Vec<_>>()
        {
            let gap = &document.text[span.end..start];

            if !gap.trim().is_empty() || gap.bytes().filter(|byte| *byte == b'\n').count() > 1 {
                break;
            }

            parts.push(body.as_str());
            start = span.start;
        }

        parts.reverse();
        let text = parts.join("\n");

        (!text.trim().is_empty()).then_some(text)
    }

    pub(crate) fn callee(&self, offset: usize) -> Option<usize> {
        self.callees
            .iter()
            .filter(|(call, _)| call.start <= offset && offset <= call.end)
            .min_by_key(|(call, _)| call.end - call.start)
            .map(|(_, callee)| callee.end.saturating_sub(1))
    }

    pub(crate) fn insertion(&self, document: &Document, offset: usize) -> (usize, String) {
        let mut insertion = self
            .dependencies
            .iter()
            .filter(|dependency| dependency.end < offset)
            .map(|dependency| dependency.end)
            .max()
            .unwrap_or(self.insertion.min(offset));

        if insertion > 0
            && document.text.as_bytes().get(insertion - 1) != Some(&b'\n')
            && let Some(end) = document.text[insertion..offset].find('\n')
        {
            insertion += end + 1;
        }

        let prefix = if insertion > 0 && document.text.as_bytes().get(insertion - 1) != Some(&b'\n')
        {
            "\n"
        } else {
            ""
        };

        (insertion, prefix.to_owned())
    }

    pub(crate) fn service_insertion(
        &self,
        document: &Document,
        offset: usize,
        service: &str,
        statement: &str,
    ) -> (usize, String) {
        let mut position = self.insertion.min(offset);

        for dependency in &self.dependencies {
            if dependency.start < position
                || dependency.end >= offset
                || !document.text[position..dependency.start].trim().is_empty()
                || dependency
                    .service
                    .as_deref()
                    .is_none_or(|name| name >= service)
            {
                break;
            }

            position = dependency.end;
            let tail = &document.text[position..offset];

            if let Some(newline) = tail.find('\n')
                && (tail[..newline].trim().is_empty()
                    || tail[..newline].trim_start().starts_with("--"))
            {
                position += newline + 1;
            }
        }

        let prefix = if position > 0 && document.text.as_bytes()[position - 1] != b'\n' {
            "\n"
        } else {
            ""
        };

        let before_require = self.dependencies.iter().any(|dependency| {
            dependency.start >= position
                && dependency.service.is_none()
                && document.text[position..dependency.start].trim().is_empty()
        });

        let separator = if before_require && !document.text[position..].starts_with('\n') {
            "\n"
        } else {
            ""
        };

        (position, format!("{prefix}{statement}{separator}"))
    }

    pub(crate) fn colors(&self, document: &Document) -> Value {
        json!(self.colors.iter().map(|(span, color, _)| json!({"range": document.range(span.start, span.end), "color": {"red": color[0], "green": color[1], "blue": color[2], "alpha": 1.0}})).collect::<Vec<_>>())
    }

    pub(crate) fn presentation(
        &self,
        document: &Document,
        range: tower_lsp_server::ls_types::Range,
        color: tower_lsp_server::ls_types::Color,
    ) -> Value {
        let Some((_, _, format)) = self
            .colors
            .iter()
            .find(|(span, _, _)| document.range(span.start, span.end) == range)
        else {
            return json!([]);
        };

        if !(1.0..=1.0).contains(&color.alpha)
            || [color.red, color.green, color.blue]
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return json!([]);
        }

        let rgb = [
            f64::from(color.red),
            f64::from(color.green),
            f64::from(color.blue),
        ];

        let label = if format.starts_with("hex") {
            let channels = rgb.map(|channel| {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "finite channels are validated in 0..=1 above; rounding yields 0..=255"
                )]
                let byte = (channel * 255.0).round() as u8;

                byte
            });

            let hex = if format.contains('S') && channels.iter().all(|value| value % 17 == 0) {
                channels.map(|value| format!("{:x}", value / 17)).join("")
            } else {
                channels.map(|value| format!("{value:02x}")).join("")
            };

            let hex = if format.contains('U') {
                hex.to_uppercase()
            } else {
                hex
            };

            let quote = if format.contains('\'') { '\'' } else { '"' };

            format!(
                "Color3.fromHex({quote}{}{hex}{quote})",
                if format.contains('#') { "#" } else { "" }
            )
        } else {
            let values = match format.as_str() {
                "fromRGB" => rgb.map(|channel| (channel * 255.0).round()),
                "fromHSV" => rgb_to_hsv(rgb),
                _ => rgb,
            };

            format!(
                "Color3.{format}({})",
                values.map(|value| value.to_string()).join(", ")
            )
        };

        json!([{"label": label, "textEdit": {"range": range, "newText": label}}])
    }
}

fn number(node: View<'_, '_>) -> Option<f64> {
    match node.parts() {
        Some(Parts::Unary { operator, operand }) if operator.text() == b"-" => {
            number(operand).map(|value| -value)
        }

        Some(Parts::Group { expression }) => number(expression),

        Some(Parts::Binary {
            left,
            operator,
            right,
        }) => {
            let left = number(left)?;
            let right = number(right)?;

            let value = match operator.text() {
                b"+" => left + right,
                b"-" => left - right,
                b"*" => left * right,
                b"/" => left / right,
                _ => return None,
            };

            value.is_finite().then_some(value)
        }

        _ if node.kind() == Kind::Number => {
            let value = text(node).replace('_', "");

            if let Some(hex) = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
            {
                u32::from_str_radix(hex, 16).ok().map(f64::from)
            } else {
                value.parse().ok()
            }
        }

        _ => None,
    }
}

fn color_call(node: View<'_, '_>) -> Option<([f64; 3], String)> {
    let Parts::Call { callee, arguments } = node.parts()? else {
        return None;
    };

    let Parts::Field { receiver, name } = callee.parts()? else {
        return None;
    };

    if receiver.text() != b"Color3" {
        return None;
    }

    let arguments: Vec<_> = arguments.children().collect();
    let format = text(name);

    if format == "fromHex" && arguments.len() == 1 {
        let value = instar_core::string_value(arguments[0].text()).ok()?;
        let hex = value.strip_prefix('#').unwrap_or(&value);

        if !matches!(hex.len(), 3 | 6) || !hex.is_ascii() {
            return None;
        }

        let step = hex.len() / 3;

        let values = [0, step, step * 2].map(|offset| {
            u8::from_str_radix(&hex[offset..offset + step], 16)
                .ok()
                .map(|value| f64::from(if step == 1 { value * 17 } else { value }) / 255.0)
        });

        return Some((
            [values[0]?, values[1]?, values[2]?],
            format!(
                "hex{}{}{}{}",
                if value.starts_with('#') { "#" } else { "" },
                if step == 1 { "S" } else { "" },
                if hex.bytes().any(|byte| byte.is_ascii_uppercase()) {
                    "U"
                } else {
                    ""
                },
                if arguments[0].text().starts_with(b"'") {
                    "'"
                } else {
                    "\""
                }
            ),
        ));
    }

    if arguments.len() > 3 || (format == "fromHSV" && arguments.len() != 3) {
        return None;
    }

    let mut color = [0.0; 3];

    for (component, argument) in color.iter_mut().zip(arguments) {
        *component = if argument.text() == b"nil" && format != "fromHSV" {
            0.0
        } else {
            number(argument)?
        };
    }

    match format.as_str() {
        "new" => {}
        "fromRGB" => color = color.map(|value| value / 255.0),

        "fromHSV" => {
            if color.iter().any(|value| !(0.0..=1.0).contains(value)) {
                return None;
            }

            color = hsv_to_rgb(color);
        }

        _ => return None,
    }

    color
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .then_some((color, format))
}

fn hsv_to_rgb([h, s, v]: [f64; 3]) -> [f64; 3] {
    [0.0, 4.0, 2.0].map(|offset| {
        let k = (h * 6.0 + offset) % 6.0;

        v * (1.0 - s * k.min(4.0 - k).clamp(0.0, 1.0))
    })
}

fn rgb_to_hsv([r, g, b]: [f64; 3]) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let h = if delta == 0.0 {
        0.0
    } else if r >= g && r >= b {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if g >= b {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };

    [h, if max == 0.0 { 0.0 } else { delta / max }, max]
}

fn comments(tree: &Tree<'_>) -> Vec<(Span, String)> {
    let mut comments = Vec::new();

    for token in &tree.tokens {
        if matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            let raw = String::from_utf8_lossy(token.span.bytes(tree.source));

            if raw.starts_with("--!") {
                continue;
            }

            let body = if matches!(token.kind, TokenKind::BlockComment) {
                let opening = raw[2..].find('[').map_or(0, |index| index + 3);

                let equal = raw[opening..]
                    .bytes()
                    .take_while(|byte| *byte == b'=')
                    .count();

                raw.get(opening + equal + 1..raw.len().saturating_sub(equal + 2))
                    .unwrap_or("")
                    .trim()
                    .to_owned()
            } else {
                raw.trim_start_matches('-').trim_start().to_owned()
            };

            comments.push((token.span, body));
        }
    }

    comments
}

fn dependencies(statements: &[View<'_, '_>]) -> Vec<Dependency> {
    let mut dependencies = Vec::new();

    for node in statements {
        if let Some(Parts::Local { bindings, values }) = node.parts() {
            for (binding, value) in bindings.zip(values) {
                let Some(Parts::Binding { name, .. }) = binding.parts() else {
                    continue;
                };

                let dependency = match value.parts() {
                    Some(Parts::Call { callee, arguments }) if callee.text() == b"require" => {
                        arguments.children().next().map(|argument| (argument, None))
                    }

                    Some(Parts::MethodCall {
                        receiver,
                        method,
                        arguments,
                        ..
                    }) if receiver.text() == b"game" && method.text() == b"GetService" => {
                        arguments.children().next().and_then(|argument| {
                            instar_core::string_value(argument.text())
                                .ok()
                                .map(|service| (argument, Some(service)))
                        })
                    }

                    _ => None,
                };

                if let Some((argument, service)) = dependency {
                    dependencies.push(Dependency {
                        name: text(name),
                        argument: argument.span(),
                        service,
                        start: node.span().start,
                        end: node.span().end,
                    });
                }
            }
        }
    }

    dependencies
}

fn exports(statements: &[View<'_, '_>]) -> BTreeSet<String> {
    let mut exports = BTreeSet::new();

    let Some(Parts::Return { mut values }) = statements.last().and_then(|node| node.parts()) else {
        return exports;
    };

    let Some(value) = values.next() else {
        return exports;
    };

    let mut returned = value;

    if value.kind() == Kind::Name {
        for node in statements {
            if let Some(Parts::Local { bindings, values }) = node.parts() {
                for (binding, initializer) in bindings.zip(values) {
                    if matches!(binding.parts(), Some(Parts::Binding { name, .. }) if name.text() == value.text())
                    {
                        returned = initializer;
                    }
                }
            }

            if let Some(Parts::Function {
                name: Some(name), ..
            }) = node.parts()
            {
                let prefix = format!("{}.", text(value));

                if let Some(member) = text(name).strip_prefix(&prefix).and_then(binding_name) {
                    exports.insert(member);
                }
            }

            if let Some(Parts::Assignment { targets, .. }) = node.parts() {
                for target in targets {
                    if let Some(Parts::Field { receiver, name }) = target.parts()
                        && receiver.text() == value.text()
                    {
                        exports.insert(text(name));
                    }
                }
            }
        }
    }

    if returned.kind() == Kind::Table {
        for field in returned.children() {
            if let Some(Parts::TableField {
                key: Some(key),
                indexed: false,
                ..
            }) = field.parts()
            {
                exports.insert(text(key));
            }
        }
    }

    exports
}
