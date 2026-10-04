use std::collections::BTreeSet;

use tower_lsp_server::ls_types::{Color, ColorInformation, ColorPresentation, Range, TextEdit};

use vermis::{
    token::{Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

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

fn text(tree: &Tree<'_>, node: NodeIndex) -> String {
    String::from_utf8_lossy(tree.text(node)).into_owned()
}

pub(crate) fn binding_name(value: &str) -> Option<String> {
    instar_core::identifier(value).then(|| value.to_owned())
}

fn top_level(tree: &Tree<'_>) -> Vec<NodeIndex> {
    let NodeKind::Root { block, .. } = tree.node(tree.root).kind else {
        return Vec::new();
    };

    tree.children(block)
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
            .map_or(tree.source.len(), |node| tree.node(*node).span.start);

        for node in &tree.nodes {
            match &node.kind {
                NodeKind::Binding { name, .. } => {
                    result.names.insert(text(tree, *name));
                }

                NodeKind::Function {
                    name: Some(name), ..
                } if matches!(tree.node(*name).kind, NodeKind::Name { .. }) => {
                    result.names.insert(text(tree, *name));
                }

                NodeKind::Call { callee, .. } => {
                    result.callees.push((node.span, tree.node(*callee).span));
                }

                NodeKind::MethodCall { method, .. } => {
                    result.callees.push((node.span, tree.node(*method).span));
                }

                NodeKind::Assignment { targets, .. } => {
                    for target in tree.list(targets) {
                        if matches!(tree.node(target.node).kind, NodeKind::Name { .. }) {
                            result.names.insert(text(tree, target.node));
                        }
                    }
                }

                NodeKind::CompoundAssignment { target, .. }
                    if matches!(tree.node(*target).kind, NodeKind::Name { .. }) =>
                {
                    result.names.insert(text(tree, *target));
                }

                _ => {}
            }

            if let NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } = &node.kind
            {
                let bindings = tree.list(bindings);
                let values = tree.list(values);

                for (binding, value) in bindings.iter().zip(values) {
                    if let NodeKind::Binding { name, .. } = tree.node(binding.node).kind
                        && matches!(tree.node(value.node).kind, NodeKind::Call { callee, .. } if tree.text(callee) == b"require")
                    {
                        result.require_bindings.push(tree.node(name).span);
                    }
                }

                if bindings.len() == 1
                    && values.len() <= 1
                    && let NodeKind::Binding { name, .. } = tree.node(bindings[0].node).kind
                {
                    let replacement =
                        values.first().map_or(Some(String::new()), |value| {
                            match tree.node(value.node).kind {
                                NodeKind::Call { .. } | NodeKind::MethodCall { .. } => {
                                    Some(text(tree, value.node))
                                }

                                NodeKind::Number { .. }
                                | NodeKind::String { .. }
                                | NodeKind::Function { .. }
                                | NodeKind::Nil { .. }
                                | NodeKind::Boolean { .. } => Some(String::new()),

                                _ => None,
                            }
                        });

                    result.locals.push(Local {
                        name: tree.node(name).span,
                        statement: node.span,
                        replacement,
                    });
                }
            }
        }

        result.dependencies = dependencies(tree, &statements);
        result.exports = exports(tree, &statements);

        if !result.names.contains("Color3") {
            for (index, node) in tree.nodes.iter().enumerate() {
                if let Some(color) = color_call(tree, NodeIndex::new(index)) {
                    result.colors.push((node.span, color.0, color.1));
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

    pub(crate) fn colors(&self, document: &Document) -> Vec<ColorInformation> {
        self.colors
            .iter()
            .map(|(span, color, _)| {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "LSP uses f32 channels; cached colors are finite in 0..=1"
                )]
                let [red, green, blue] = color.map(|value| value as f32);

                ColorInformation {
                    range: document.range(span.start, span.end),
                    color: Color {
                        red,
                        green,
                        blue,
                        alpha: 1.0,
                    },
                }
            })
            .collect()
    }

    pub(crate) fn presentation(
        &self,
        document: &Document,
        range: Range,
        color: Color,
    ) -> Vec<ColorPresentation> {
        let Some((_, _, format)) = self
            .colors
            .iter()
            .find(|(span, _, _)| document.range(span.start, span.end) == range)
        else {
            return Vec::new();
        };

        if !(1.0..=1.0).contains(&color.alpha)
            || [color.red, color.green, color.blue]
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Vec::new();
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

        vec![ColorPresentation {
            text_edit: Some(TextEdit {
                range,
                new_text: label.clone(),
            }),
            label,
            additional_text_edits: None,
        }]
    }
}

fn number(tree: &Tree<'_>, node: NodeIndex) -> Option<f64> {
    match tree.node(node).kind {
        NodeKind::Unary { operator, operand }
            if tree.token(operator).kind == TokenKind::Symbol(Symbol::Subtract) =>
        {
            number(tree, operand).map(|value| -value)
        }

        NodeKind::Group { expression, .. } => number(tree, expression),

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let left = number(tree, left)?;
            let right = number(tree, right)?;

            let value = match tree.token(operator).kind {
                TokenKind::Symbol(Symbol::Add) => left + right,
                TokenKind::Symbol(Symbol::Subtract) => left - right,
                TokenKind::Symbol(Symbol::Multiply) => left * right,
                TokenKind::Symbol(Symbol::Divide) => left / right,
                _ => return None,
            };

            value.is_finite().then_some(value)
        }

        NodeKind::Number { .. } => {
            let value = text(tree, node).replace('_', "");

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

fn color_call(tree: &Tree<'_>, node: NodeIndex) -> Option<([f64; 3], String)> {
    let NodeKind::Call { callee, arguments } = tree.node(node).kind else {
        return None;
    };

    let NodeKind::Field { receiver, name, .. } = tree.node(callee).kind else {
        return None;
    };

    if tree.text(receiver) != b"Color3" {
        return None;
    }

    let arguments = tree.children(arguments);
    let format = text(tree, name);

    if format == "fromHex" && arguments.len() == 1 {
        let value = instar_core::string_value(tree.text(arguments[0])).ok()?;
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
                if tree.text(arguments[0]).starts_with(b"'") {
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
        *component = if tree.text(argument) == b"nil" && format != "fromHSV" {
            0.0
        } else {
            number(tree, argument)?
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

fn dependencies(tree: &Tree<'_>, statements: &[NodeIndex]) -> Vec<Dependency> {
    let mut dependencies = Vec::new();

    for node in statements {
        if let NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        } = &tree.node(*node).kind
        {
            for (binding, value) in tree.list(bindings).iter().zip(tree.list(values)) {
                let NodeKind::Binding { name, .. } = tree.node(binding.node).kind else {
                    continue;
                };

                let dependency = match tree.node(value.node).kind {
                    NodeKind::Call { callee, arguments } if tree.text(callee) == b"require" => tree
                        .children(arguments)
                        .first()
                        .copied()
                        .map(|argument| (argument, None)),

                    NodeKind::MethodCall {
                        receiver,
                        method,
                        arguments,
                        ..
                    } if tree.text(receiver) == b"game" && tree.text(method) == b"GetService" => {
                        tree.children(arguments)
                            .first()
                            .copied()
                            .and_then(|argument| {
                                instar_core::string_value(tree.text(argument))
                                    .ok()
                                    .map(|service| (argument, Some(service)))
                            })
                    }

                    _ => None,
                };

                if let Some((argument, service)) = dependency {
                    dependencies.push(Dependency {
                        name: text(tree, name),
                        argument: tree.node(argument).span,
                        service,
                        start: tree.node(*node).span.start,
                        end: tree.node(*node).span.end,
                    });
                }
            }
        }
    }

    dependencies
}

fn exports(tree: &Tree<'_>, statements: &[NodeIndex]) -> BTreeSet<String> {
    let mut exports = BTreeSet::new();

    let Some(last) = statements.last() else {
        return exports;
    };

    let NodeKind::Return { ref values, .. } = tree.node(*last).kind else {
        return exports;
    };

    let Some(value) = tree.list(values).first().map(|entry| entry.node) else {
        return exports;
    };

    let mut returned = value;

    if matches!(tree.node(value).kind, NodeKind::Name { .. }) {
        for node in statements {
            if let NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } = &tree.node(*node).kind
            {
                for (binding, initializer) in tree.list(bindings).iter().zip(tree.list(values)) {
                    if matches!(tree.node(binding.node).kind, NodeKind::Binding { name, .. } if tree.text(name) == tree.text(value))
                    {
                        returned = initializer.node;
                    }
                }
            }

            if let NodeKind::Function {
                name: Some(name), ..
            } = tree.node(*node).kind
                && let NodeKind::FunctionName {
                    ref path,
                    method: None,
                    ..
                } = tree.node(name).kind
                && let [receiver, member] = tree.list(path)
                && tree.text(receiver.node) == tree.text(value)
                && let Some(member) = binding_name(&text(tree, member.node))
            {
                exports.insert(member);
            }

            if let NodeKind::Assignment { ref targets, .. } = tree.node(*node).kind {
                for target in tree.list(targets) {
                    if let NodeKind::Field { receiver, name, .. } = tree.node(target.node).kind
                        && tree.text(receiver) == tree.text(value)
                    {
                        exports.insert(text(tree, name));
                    }
                }
            }

            if let NodeKind::CompoundAssignment { target, .. } = tree.node(*node).kind
                && let NodeKind::Field { receiver, name, .. } = tree.node(target).kind
                && tree.text(receiver) == tree.text(value)
            {
                exports.insert(text(tree, name));
            }
        }
    }

    if let NodeKind::Table { ref fields, .. } = tree.node(returned).kind {
        for field in tree.list(fields) {
            if let NodeKind::TableField {
                key: Some(key),
                opening: None,
                ..
            } = tree.node(field.node).kind
            {
                exports.insert(text(tree, key));
            }
        }
    }

    exports
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Position;

    #[test]
    fn exports_use_parsed_function_paths() {
        let source = concat!(
            "local Module = { value = true }\n",
            "function Module.compact() end\n",
            "function Module . spaced() end\n",
            "function Module --[[ owner ]] . --[[ member ]] commented() end\n",
            "function Module.nested.indirect() end\n",
            "function Module.nested:method() end\n",
            "function Other.unrelated() end\n",
            "Module.assigned = true\n",
            "return Module\n",
        );

        let tree = vermis::parse(source.as_bytes());
        assert!(tree.diagnostics.is_empty(), "{:?}", tree.diagnostics);

        assert_eq!(
            Syntax::new(&tree).exports,
            ["assigned", "commented", "compact", "spaced", "value"]
                .map(str::to_owned)
                .into()
        );
    }

    #[test]
    fn typed_colors_and_presentations_keep_ranges_formats_and_validation() {
        let document = Document::new(
            crate::document::uri(&std::env::temp_dir().join("colors.luau")).unwrap(),
            1,
            "local first = Color3.fromHex('#ABC')\nlocal second = Color3.fromRGB(255, 0, 128)\n"
                .into(),
        )
        .unwrap();

        let syntax = document.features();
        let colors = syntax.colors(&document);
        assert_eq!(colors.len(), 2);

        assert_eq!(
            colors[0],
            ColorInformation {
                range: Range::new(Position::new(0, 14), Position::new(0, 36)),
                color: Color {
                    red: 170.0 / 255.0,
                    green: 187.0 / 255.0,
                    blue: 204.0 / 255.0,
                    alpha: 1.0,
                },
            }
        );

        assert_eq!(
            colors[1].color,
            Color {
                red: 1.0,
                green: 0.0,
                blue: 128.0 / 255.0,
                alpha: 1.0
            }
        );

        for (color, label) in colors
            .iter()
            .zip(["Color3.fromHex('#ABC')", "Color3.fromRGB(255, 0, 128)"])
        {
            assert_eq!(
                syntax.presentation(&document, color.range, color.color),
                vec![ColorPresentation {
                    label: label.into(),
                    text_edit: Some(TextEdit {
                        range: color.range,
                        new_text: label.into()
                    }),
                    additional_text_edits: None,
                }]
            );
        }

        for color in [
            Color {
                alpha: 0.5,
                ..colors[0].color
            },
            Color {
                alpha: f32::NAN,
                ..colors[0].color
            },
            Color {
                red: f32::INFINITY,
                ..colors[0].color
            },
            Color {
                green: -0.1,
                ..colors[0].color
            },
            Color {
                blue: 1.1,
                ..colors[0].color
            },
        ] {
            assert_eq!(
                syntax.presentation(&document, colors[0].range, color),
                Vec::<ColorPresentation>::new()
            );
        }

        let missing = Range::new(Position::new(0, 0), Position::new(0, 1));

        assert_eq!(
            syntax.presentation(&document, missing, colors[0].color),
            Vec::<ColorPresentation>::new()
        );
    }
}
