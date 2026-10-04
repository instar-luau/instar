use crate::config::{FormatOptions, IndentStyle, LineEnding, Semicolons, TrailingComma};

use super::{
    layout::{Delimiter, Layout, Line},
    preparation::{Kind, Prepared, enclosing_brace, type_table},
};

struct Renderer<'a> {
    prepared: &'a Prepared,
    options: &'a FormatOptions,
    output: String,
    newline: &'static str,
    level: usize,
    line_width: usize,
    delimiters: Vec<Delimiter>,
    parameter_depth: usize,
    previous: Option<usize>,
    previous_code: Option<(usize, usize)>,
    blocks: Vec<bool>,
}

pub(super) fn render(prepared: &Prepared, options: &FormatOptions) -> String {
    let mut renderer = Renderer {
        prepared,
        options,
        output: String::new(),
        newline: if options.line_ending == LineEnding::CrLf {
            "\r\n"
        } else {
            "\n"
        },
        level: 0,
        line_width: 0,
        delimiters: Vec::new(),
        parameter_depth: 0,
        previous: None,
        previous_code: None,
        blocks: Vec::new(),
    };

    let mut layout = Layout::new(prepared, options);

    for index in 0..prepared.tokens.len() {
        let expression_start = layout.expression_start(index);
        let current_if_expression = expression_start.is_some();
        renderer.close_indentation(index, current_if_expression);

        let opening = layout.delimiter(index, renderer.previous, renderer.line_width);

        let line = layout.line(
            index,
            renderer.previous,
            renderer.line_width,
            renderer.delimiters.last(),
            opening.as_ref(),
            expression_start,
        );

        let open_wrapped = opening.as_ref().is_some_and(|delimiter| delimiter.wrapped);

        if let Some(opening) = opening {
            renderer.parameter_depth += usize::from(opening.parameters);
            renderer.delimiters.push(opening);
        }

        renderer.emit_prefix(index, &line, current_if_expression, &layout);

        if renderer.omit_trailing_comma(index) {
            renderer.previous = Some(index);
            continue;
        }

        renderer.emit_token(index);
        renderer.after_token(index, open_wrapped, current_if_expression);
        renderer.previous = Some(index);
    }

    renderer.finish()
}

impl Renderer<'_> {
    fn close_indentation(&mut self, index: usize, current_if_expression: bool) {
        let text = self.prepared.tokens[index].text.as_str();

        let block_closer = matches!(text, "end" | "until")
            || (matches!(text, "else" | "elseif") && !current_if_expression);

        if block_closer && self.blocks.pop().unwrap_or(false) {
            self.level = self.level.saturating_sub(1);
        }

        if matches!(text, ")" | "}" | "]")
            && self
                .delimiters
                .last()
                .is_some_and(|delimiter| delimiter.wrapped)
        {
            self.level = self.level.saturating_sub(1);
        }
    }

    fn break_line(&mut self) {
        self.output.push_str(self.newline);
        self.line_width = 0;
    }

    fn emit_blank_line(&mut self, index: usize, current_if_expression: bool) {
        let token = &self.prepared.tokens[index];

        if token.newlines <= 1 || self.output.is_empty() {
            return;
        }

        let in_table = self
            .delimiters
            .last()
            .is_some_and(|delimiter| delimiter.kind == '{');

        let block_edge = !current_if_expression
            && (self
                .prepared
                .syntax
                .edges
                .binary_search(&token.start)
                .is_ok()
                || matches!(token.text.as_str(), "end" | "else" | "elseif" | "until"));

        let preserve_gap = if in_table {
            self.options.tables.blank_lines == crate::config::TableBlankLines::Preserve
        } else if block_edge {
            self.options.blocks.edge_blank_lines == crate::config::EdgeBlankLines::Preserve
        } else {
            true
        };

        if preserve_gap {
            if !self.output.ends_with('\n') {
                self.break_line();
            }

            self.break_line();
        }
    }

    fn emit_prefix(
        &mut self,
        index: usize,
        line: &Line,
        current_if_expression: bool,
        layout: &Layout<'_>,
    ) {
        let tokens = &self.prepared.tokens;
        let syntax = &self.prepared.syntax;

        if tokens[index].text == "}"
            && self
                .delimiters
                .last()
                .is_some_and(|delimiter| delimiter.kind == '{' && delimiter.wrapped)
            && self.options.tables.trailing_comma == TrailingComma::Multiline
            && !enclosing_brace(tokens, index)
                .is_some_and(|start| type_table(syntax, &tokens[start]))
            && let Some((previous, position)) = self.previous_code
            && !matches!(tokens[previous].text.as_str(), "," | "{")
        {
            self.output.insert(position, ',');
        }

        if line.break_before && !self.output.is_empty() && !self.output.ends_with('\n') {
            self.break_line();
        }

        self.emit_blank_line(index, current_if_expression);

        if self.output.ends_with('\n') || self.output.is_empty() {
            let visual_level = self.level.saturating_sub(self.parameter_depth)
                + usize::try_from(self.prepared.return_levels.get(index).copied().unwrap_or(0))
                    .expect("return continuation is nonnegative")
                + usize::try_from(line.conditional_depth)
                    .expect("conditional continuation is nonnegative")
                + if line.break_before {
                    line.continuation
                } else {
                    0
                };

            match self.options.indent_style {
                IndentStyle::Tabs => self.output.extend(std::iter::repeat_n('\t', visual_level)),

                IndentStyle::Spaces => self.output.extend(std::iter::repeat_n(
                    ' ',
                    visual_level.saturating_mul(self.options.indent_width),
                )),
            }

            self.line_width = visual_level.saturating_mul(self.options.indent_width);
        } else if let Some(previous) = self.previous
            && layout.space(previous, index)
        {
            self.output.push(' ');
            self.line_width += 1;
        }
    }

    fn omit_trailing_comma(&self, index: usize) -> bool {
        self.prepared.tokens[index].text == ","
            && self
                .delimiters
                .last()
                .is_some_and(|delimiter| delimiter.kind == '{')
            && self.options.tables.trailing_comma == TrailingComma::Never
            && self
                .prepared
                .tokens
                .iter()
                .skip(index + 1)
                .find(|token| token.kind != Kind::Comment)
                .is_some_and(|next| next.text == "}")
    }

    fn emit_token(&mut self, index: usize) {
        let tokens = &self.prepared.tokens;
        let text = tokens[index].text.as_str();
        self.output.push_str(text);
        self.line_width += text.len();

        if self.options.semicolons == Semicolons::Always
            && self.prepared.statement_end[index]
            && text != ";"
            && tokens.get(index + 1).is_none_or(|next| next.text != ";")
        {
            self.output.push(';');
        }

        if tokens[index].kind != Kind::Comment {
            self.previous_code = Some((index, self.output.len()));
        }
    }

    fn after_token(&mut self, index: usize, open_wrapped: bool, current_if_expression: bool) {
        let tokens = &self.prepared.tokens;
        let token = &tokens[index];
        let text = token.text.as_str();

        if open_wrapped {
            self.level += 1;
        }

        if text == ";" && enclosing_brace(tokens, index).is_none() {
            if self.options.semicolons == Semicolons::Necessary
                && !tokens
                    .get(index + 1)
                    .is_some_and(|next| matches!(next.text.as_str(), "(" | "["))
            {
                self.output.pop();
            }

            if !self.output.ends_with('\n') {
                self.break_line();
            }
        }

        if (text == "function" && self.prepared.declared.get(index) != Some(&true))
            || matches!(text, "do" | "repeat")
            || (text == "with"
                && self
                    .prepared
                    .syntax
                    .class_headers
                    .iter()
                    .any(|&(start, end)| start <= token.start && token.end <= end))
            || (matches!(text, "then" | "else") && !current_if_expression)
        {
            self.blocks.push(true);
            self.level += 1;
        }

        if matches!(text, ")" | "}" | "]")
            && self
                .delimiters
                .pop()
                .is_some_and(|delimiter| delimiter.parameters)
        {
            self.parameter_depth -= 1;
        }

        if token.kind == Kind::Comment && !self.output.ends_with('\n') {
            self.break_line();
        }
    }

    fn finish(mut self) -> String {
        self.output
            .truncate(self.output.trim_end_matches([' ', '\t', '\r', '\n']).len());

        if self.options.final_newline && !self.output.is_empty() {
            self.output.push_str(self.newline);
        }

        self.output
    }
}
