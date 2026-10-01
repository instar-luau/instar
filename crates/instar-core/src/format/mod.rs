//! Luau source formatting.

use std::{borrow::Cow, io};

use crate::config::FormatOptions;

mod layout;
mod preparation;
mod render;
mod requires;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Formats Luau source according to the project format options.
/// # Errors
/// Returns a syntax or require-ordering error for invalid or unformattable source.
pub fn source(input: &str, options: &FormatOptions) -> io::Result<String> {
    let parsed = vermis::parse(input.as_bytes());

    if let Some(error) = parsed.diagnostics.first() {
        return Err(invalid(format!(
            "{} at byte {}",
            error.message, error.span.start
        )));
    }

    let ordered = requires::sort(input, &options.requires, &parsed)?;
    let reparsed;

    let parsed_ordered = match &ordered {
        Cow::Borrowed(_) => &parsed,

        Cow::Owned(source) => {
            reparsed = vermis::parse(source.as_bytes());

            &reparsed
        }
    };

    let prepared = preparation::prepare(parsed_ordered, options)?;
    let formatted = render::render(&prepared, options);

    if let Some(error) = vermis::parse(formatted.as_bytes()).diagnostics.first() {
        return Err(invalid(format!(
            "formatter produced invalid Luau at byte {}: {}",
            error.span.start, error.message
        )));
    }

    Ok(formatted)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{
        BeforeFunctionParentheses, CallParentheses, IndentStyle, LineEnding, QuoteStyle,
        TrailingComma, Wrap,
    };

    #[test]
    fn preserves_literal_line_endings_under_both_output_policies() {
        let literals = [
            "\"a\\\r\nb\\\nc\\\rd\"",
            "'a\\\r\nb\\\nc\\\rd'",
            "\"a\\z \r\n\tb\\z \n c\"",
            "[[a\r\nb\nc\rd\r\r\ne]]",
            "[==[\r\na\r\nb\nc\rd\r\r\ne]==]",
            "`a\\\r\nb\\\nc\\\rd`",
            "`a\\\r\n{count}b\\\n{count}c\\\rd`",
        ];

        for line_ending in [LineEnding::Lf, LineEnding::CrLf] {
            let newline = if line_ending == LineEnding::CrLf {
                "\r\n"
            } else {
                "\n"
            };

            for final_newline in [false, true] {
                let options = FormatOptions {
                    line_ending,
                    final_newline,
                    quote_style: QuoteStyle::Preserve,
                    ..FormatOptions::default()
                };

                for literal in literals {
                    let input = format!(" \tlocal text={literal} \t\r\n\n");

                    let expected = format!(
                        "local text = {literal}{}",
                        if final_newline { newline } else { "" }
                    );

                    let output = source(&input, &options).unwrap();
                    assert_eq!(output, expected);
                    assert_eq!(source(&output, &options).unwrap(), output);
                }
            }
        }
    }

    #[test]
    fn applies_line_endings_to_surrounding_formatted_whitespace() {
        let input = concat!(
            "-- lead\r\n",
            "local values={\r\n",
            "[=[a\r\nb\nc]=],\n",
            "}\r\n\r\n",
            "local show=print\n",
            "show(\"one\");(show)(\"two\")\r\n",
            "-- tail \t\r\n\n",
        );

        for line_ending in [LineEnding::Lf, LineEnding::CrLf] {
            let newline = if line_ending == LineEnding::CrLf {
                "\r\n"
            } else {
                "\n"
            };

            for final_newline in [false, true] {
                let options = FormatOptions {
                    line_ending,
                    final_newline,
                    ..FormatOptions::default()
                };

                let expected = [
                    "-- lead",
                    "local values = {",
                    "\t[=[a\r\nb\nc]=],",
                    "}",
                    "",
                    "local show = print",
                    "show(\"one\");",
                    "(show)(\"two\")",
                    "-- tail",
                ]
                .join(newline)
                    + if final_newline { newline } else { "" };

                let output = source(input, &options).unwrap();
                assert_eq!(output, expected);
                assert_eq!(source(&output, &options).unwrap(), output);
            }
        }
    }

    #[test]
    fn preserves_require_comments_and_separators_across_reformatting() {
        let options = FormatOptions {
            line_ending: LineEnding::CrLf,
            ..FormatOptions::default()
        };

        let input = "--!strict\nlocal z = require(\n\"./z\"\n) -- z\n-- a\nlocal a = require(\"@a\")\nlocal f = print\nf(\"one\");(f)(\"two\")\n";
        let output = source(input, &options).unwrap();
        assert!(output.starts_with("--!strict\r\n-- a\r\nlocal a = require(\"@a\")\r\n"));
        assert!(output.contains("local z = require(\"./z\") -- z\r\n"));
        assert!(output.contains("f(\"one\");\r\n(f)(\"two\")\r\n"));
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn omits_literal_call_parentheses_without_touching_declarations() {
        let mut options = FormatOptions::default();
        options.calls.parentheses = CallParentheses::OmitLiteral;
        let input = "function show(value) return value end\nlocal a = show(\"hello\")\nlocal b = show({ value = 1 })\n";
        let output = source(input, &options).unwrap();

        assert!(output.contains("function show(value)"), "{output}");
        assert!(output.contains("show \"hello\""), "{output}");
        assert!(output.contains("show { value = 1 }"), "{output}");
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn never_wrap_collapses_type_operators_and_named_calls() {
        let mut options = FormatOptions::default();
        options.types.operator_wrap = Wrap::Never;
        options.chains.wrap = Wrap::Never;
        let input = "type Either = Left\n | Right\nlocal result = object:first()\n:second()\n";
        let output = source(input, &options).unwrap();
        assert!(output.lines().any(|line| line.contains("Left | Right")));

        assert!(
            output
                .lines()
                .any(|line| line.contains("first") && line.contains("second"))
        );

        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn spaces_every_compound_assignment_from_the_parser() {
        let options = FormatOptions::default();

        let input = concat!(
            "local users=1\n",
            "users+=1\nusers-=2\nusers*=3\nusers/=4\n",
            "users//=5\nusers%=6\nusers^=7\nusers+=(2)\n",
            "local label=\"a\"\nlabel..=\"b\"\n",
        );

        let expected = concat!(
            "local users = 1\n",
            "users += 1\nusers -= 2\nusers *= 3\nusers /= 4\n",
            "users //= 5\nusers %= 6\nusers ^= 7\nusers += (2)\n",
            "local label = \"a\"\nlabel ..= \"b\"\n",
        );

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn keeps_unary_minus_tight_without_collapsing_binary_subtraction() {
        let options = FormatOptions::default();
        let input = "local a=-1\nlocal b=1- -2\nlocal c=#items\n";
        let expected = "local a = -1\nlocal b = 1 - -2\nlocal c = #items\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn recognizes_if_expressions_after_compound_assignment() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.if_expressions.wrap = Wrap::Always;
        options.if_expressions.placement = crate::config::IfExpressionPlacement::NextLine;
        let input = "local users = 1\nusers += if ready then 1 else 2\n";
        let output = source(input, &options).unwrap();

        assert!(output.contains("users +=\n    if ready then"), "{output}");
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_short_conditional_cast_in_function_on_one_line() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local handlers = {\n",
            "    choose = function(item): Result?\n",
            "        return if item:matches(\"Result\") then item :: Result else nil\n",
            "    end,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn spaces_cast_before_parenthesized_function_type() {
        let options = FormatOptions::default();
        let input = "(callback :: (string) -> ())(value)\n";
        let output = source(input, &options).unwrap();

        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn preserves_statement_gaps_independently_of_block_and_table_edges() {
        let input = "local a = 1\n\n\nlocal b = 2\nfunction f()\n\nlocal c = 3\n\nlocal d = 4\n\nend\nlocal t = {\na = 1,\n\n\nb = 2,\n}\n";
        let options = FormatOptions::default();
        let output = source(input, &options).unwrap();

        assert!(output.contains("local a = 1\n\nlocal b = 2\n"), "{output}");
        assert!(output.contains("local c = 3\n\n\tlocal d = 4"));
        assert!(!output.contains("function f()\n\n"));
        assert!(!output.contains("local d = 4\n\nend"));
        assert!(output.contains("a = 1,\n\n\tb = 2,"));
        assert_eq!(source(&output, &options).unwrap(), output);

        let mut preserve_edges = options.clone();
        preserve_edges.blocks.edge_blank_lines = crate::config::EdgeBlankLines::Preserve;
        let preserved = source(input, &preserve_edges).unwrap();
        assert!(preserved.contains("function f()\n\n\tlocal c = 3"));
        assert!(preserved.contains("local d = 4\n\nend"));

        let mut remove_table_gaps = options;
        remove_table_gaps.tables.blank_lines = crate::config::TableBlankLines::Remove;
        let table = source(input, &remove_table_gaps).unwrap();
        assert!(table.contains("a = 1,\n\tb = 2,"));
    }

    #[test]
    fn formats_expression_type_arguments_without_spacing_comparisons() {
        let options = FormatOptions::default();
        let input = "local x = factory<<Item?>>(nil)\nlocal less = a < b\n";
        let output = source(input, &options).unwrap();

        assert!(output.contains("factory<<Item?>>(nil)"));
        assert!(output.contains("a < b"));
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn separates_generic_closer_from_type_alias_assignment() {
        let options = FormatOptions::default();

        let input =
            "export type Box<T> = { value: T }\ndeclare function take<T>(item: Box<T>): T\n";

        let output = source(input, &options).unwrap();

        assert_eq!(
            output,
            "export type Box<T> = { value: T }\ndeclare function take<T>(item: Box<T>): T\n"
        );

        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn formats_type_functions_and_readonly_indexers() {
        let options = FormatOptions::default();
        let input = "export type AnyFunction =(...any) ->(...any)\nexport type AnyTable = { [any]: any }\n\ntype ReadonlyArray<T> = { read[number]: T }\ntype ReadonlyTable = { read[any]: unknown }\n";
        let expected = "export type AnyFunction = (...any) -> (...any)\nexport type AnyTable = { [any]: any }\n\ntype ReadonlyArray<T> = { read [number]: T }\ntype ReadonlyTable = { read [any]: unknown }\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn keeps_value_table_separators_distinct_from_type_table_separators() {
        let mut options = FormatOptions::default();
        options.types.table_separator = crate::config::TypeTableSeparator::Semicolon;

        let input =
            "type Named = { a: number, b: number }\nlocal value: Named = { a = 1, b = 2 }\n";

        let expected =
            "type Named = { a: number; b: number }\nlocal value: Named = { a = 1, b = 2 }\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn respects_inner_delimiter_spacing_without_separating_calls_or_indexes() {
        let mut options = FormatOptions::default();
        options.spacing.parentheses = true;
        options.spacing.brackets = true;
        let input = "local x = f(a)[i]\nlocal y = read[1]\n";
        let expected = "local x = f( a )[ i ]\nlocal y = read[ 1 ]\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn preserves_multiline_type_intersection_layout() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "type Registry = {\n",
            "    create: ((tag: \"A\") -> A)\n",
            "        & ((tag: \"B\") -> B)\n",
            "        & ((tag: \"C\") -> C)\n",
            "        & ((tag: \"D\") -> D)\n",
            "        & ((tag: \"E\") -> E)\n",
            "        & ((tag: \"F\") -> F)\n",
            "        & ((tag: \"G\") -> G),\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_wrapped_declaration_parameters_without_a_runtime_body() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.parameters.wrap = Wrap::Always;

        let input = concat!(
            "declare function wrap<A..., B..., C..., D...>(\n",
            "    target: (A...) -> B...,\n",
            "    replacement: (C...) -> D...\n",
            "): (A...) -> B...\n",
        );

        assert_eq!(source(input, &options).unwrap(), input);
    }

    #[test]
    fn preserves_declaration_parameters_when_return_type_exceeds_width() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let multiline = concat!(
            "declare function replace_handler<A1..., R1..., A2..., R2...>(\n",
            "    original: (A1...) -> R1...,\n",
            "    replacement: (A2...) -> R2...\n",
            "): (A1...) -> R1...\n",
            "declare function patch_method(\n",
            "    receiver: AnyTable | Instance | userdata,\n",
            "    method: string,\n",
            "    replacement: AnyFunction\n",
            "): AnyFunction\n",
        );

        let flat = concat!(
            "declare function replace_handler<A1..., R1..., A2..., R2...>(original: (A1...) -> R1..., replacement: (A2...) -> R2...): (A1...) -> R1...\n",
            "declare function patch_method(receiver: AnyTable | Instance | userdata, method: string, replacement: AnyFunction): AnyFunction\n",
        );

        assert_eq!(source(multiline, &options).unwrap(), multiline);
        assert_eq!(source(flat, &options).unwrap(), multiline);

        assert_eq!(
            source(&source(flat, &options).unwrap(), &options).unwrap(),
            multiline
        );
    }

    #[test]
    fn wraps_if_expressions_only_past_the_formatted_line_width() {
        let input = "local choice = if data[1] == data[2] and data[3] then x else y\n";

        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: input.trim_end().len(),
            ..FormatOptions::default()
        };

        let inline = source(input, &options).unwrap();
        assert_eq!(inline, input);
        assert_eq!(source(&inline, &options).unwrap(), inline);

        options.width -= 1;
        let wrapped = source(input, &options).unwrap();

        assert_eq!(
            wrapped,
            "local choice = if data[1] == data[2] and data[3] then\n    x\nelse\n    y\n"
        );

        assert_eq!(source(&wrapped, &options).unwrap(), wrapped);
    }

    #[test]
    fn preserves_multiline_binary_continuations_inside_auto_wrapped_calls() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local value = compute(function()\n",
            "    local direction = axes.RightVector * horizontal\n",
            "        + Vector3.yAxis * vertical\n",
            "        + axes.LookVector * depth\n",
            "    local content_size = node.size\n",
            "        - Vector2.new(padding.left + padding.right, padding.top + padding.bottom) * node.scale\n",
            "    return direction\n",
            "end)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_standalone_subtraction_continuation() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "function measure(node, padding)\n",
            "    const content_size = node.size\n",
            "        - Vector2.new(padding.left + padding.right, padding.top + padding.bottom) * node.scale\n",
            "    return content_size\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_leading_logical_operators_as_continuations() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "function is_ready(flag)\n",
            "    return available\n",
            "        and flag ~= false\n",
            "        and (typeof(flag) ~= \"function\" or (flag :: Reader<boolean>)())\n",
            "end\n",
        );

        assert_eq!(source(input, &options).unwrap(), input);

        assert_eq!(
            source(&source(input, &options).unwrap(), &options).unwrap(),
            input
        );
    }

    #[test]
    fn keeps_nested_if_expression_inline_inside_multiline_branch() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local function collect(dictionary, mapper)\n",
            "    local result = {}\n",
            "    for key, value in dictionary do\n",
            "        local choice = if mapper == nil then\n",
            "            if value == nil then nil else { key = key, value = value }\n",
            "        else\n",
            "            mapper(key, value)\n",
            "        if choice ~= nil then\n",
            "            table.insert(result, choice)\n",
            "        end\n",
            "    end\n",
            "    return result\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn preserves_multiline_if_expression_branch_alignment() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local bounds = if corner then\n",
            "    Vector.offset(WIDTH, WIDTH)\n",
            "elseif direction ~= 0 then\n",
            "    Vector.new(0, WIDTH, 1, 0)\n",
            "else\n",
            "    Vector.new(1, 0, 0, WIDTH)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_statement_else_after_nested_if_expression() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "if options.pick == nil then\n",
            "    getter, setter = wire.signal(if options.default ~= nil then options.default else false)\n",
            "else\n",
            "    getter, setter = options.pick, options.set\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_wrapped_chains_and_if_expressions() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.chains.wrap = Wrap::Always;
        options.if_expressions.wrap = Wrap::Always;
        options.if_expressions.placement = crate::config::IfExpressionPlacement::NextLine;
        let input = "local x = receiver:first():second():third()\nlocal flag = if condition then yes else no\n";
        let output = source(input, &options).unwrap();

        assert!(
            output.contains("receiver:first()\n    :second()\n    :third()"),
            "{output}"
        );

        assert!(
            output.contains("local flag =\n    if condition then"),
            "{output}"
        );

        assert!(
            output.contains("\n        yes\n    else\n        no\n"),
            "{output}"
        );

        assert_eq!(source(&output, &options).unwrap(), output);

        options.chains.wrap = Wrap::Auto;
        let chain = "local x = receiver:first()\n:second()\n:third()\n";
        let formatted = source(chain, &options).unwrap();

        assert_eq!(
            formatted,
            "local x = receiver:first()\n    :second()\n    :third()\n"
        );

        assert_eq!(source(&formatted, &options).unwrap(), formatted);
    }

    #[test]
    fn keeps_conditional_table_call_indented_through_nested_functions() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let input = concat!(
            "local nodes = {\n",
            "    if state.active and on_action ~= nil then\n",
            "        UI.Button({\n",
            "            Opacity = function()\n",
            "                return if hovered() then 0 else 1\n",
            "            end,\n",
            "            Text = \"×\",\n",
            "            MouseEnter = function()\n",
            "                hovered(true)\n",
            "            end,\n",
            "        })\n",
            "    else\n",
            "        nil,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_statement_else_and_elseif_branches_in_nested_conditional_call() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let input = concat!(
            "local cards = {\n",
            "    if primary then\n",
            "        UI.Card({\n",
            "            OnClick = function()\n",
            "                if ready then\n",
            "                    fire()\n",
            "                else\n",
            "                    defer()\n",
            "                end\n",
            "            end,\n",
            "        })\n",
            "    elseif secondary then\n",
            "        UI.Card({\n",
            "            Label = \"retry\",\n",
            "        })\n",
            "    else\n",
            "        nil,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_nested_table_indented_inside_multiline_return_list() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let input = concat!(
            "function render(item)\n",
            "    return\n",
            "        UI.Panel({\n",
            "            Enabled = true,\n",
            "            Opacity = function()\n",
            "                return math.clamp(item.opacity, 0, 1)\n",
            "            end,\n",
            "            item.child,\n",
            "        }),\n",
            "        EXIT_TIME\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_multiline_function_argument_inline_and_return_values_separate() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local rows = map_all(source, function(item, active)\n",
            "    return\n",
            "        make_row(item.label, active),\n",
            "        TIMEOUT\n",
            "end)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);

        let nested_table = concat!(
            "local rows = map_all(source, function(item, active)\n",
            "    return\n",
            "        Row({\n",
            "            title = item.title,\n",
            "        }),\n",
            "        TIMEOUT\n",
            "end)\n",
        );

        let output = source(nested_table, &options).unwrap();
        assert!(output.starts_with("local rows = map_all(source, function(item, active)\n"));
        assert!(output.contains("\n    return\n        Row("));
        assert!(output.contains("\n        TIMEOUT\nend)\n"));
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn hugs_last_function_argument_past_commas_in_its_body() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;
        options.width = "local rows = map_all(source, function".len();

        let input = concat!(
            "local rows = map_all(source, function()\n",
            "    return 1, 2\n",
            "end)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn hugs_multiline_final_table_argument() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let expected = concat!(
            "local result = stream.watch(context, {\n",
            "    signal = item:on(\"Change\"),\n",
            "    read = function()\n",
            "        return item.value\n",
            "    end,\n",
            "})\n",
        );

        let vertical = concat!(
            "local result = stream.watch(\n",
            "    context,\n",
            "    {\n",
            "        signal = item:on(\"Change\"),\n",
            "        read = function()\n",
            "            return item.value\n",
            "        end,\n",
            "    }\n",
            ")\n",
        );

        assert_eq!(source(expected, &options).unwrap(), expected);
        assert_eq!(source(vertical, &options).unwrap(), expected);
    }

    #[test]
    fn wraps_nested_calls_only_past_the_formatted_line_width() {
        let call = "    return Keyframe.make(parts[1], Tone.make(parts[2], parts[3], parts[4]))";
        let input = format!("function build(parts)\n{call}\nend\n");

        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: call.len(),
            ..FormatOptions::default()
        };

        let inline = source(&input, &options).unwrap();
        assert_eq!(inline, input);
        assert_eq!(source(&inline, &options).unwrap(), inline);

        options.width -= 1;
        let wrapped = source(&input, &options).unwrap();

        assert_eq!(
            wrapped,
            "function build(parts)\n    return Keyframe.make(\n        parts[1],\n        Tone.make(parts[2], parts[3], parts[4])\n    )\nend\n"
        );

        assert_eq!(source(&wrapped, &options).unwrap(), wrapped);
    }

    #[test]
    fn keeps_nested_callback_bodies_out_of_call_width() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: 40,
            ..FormatOptions::default()
        };

        let input = concat!(
            "outer(\n",
            "    inner(function()\n",
            "        if ready and active then\n",
            "            consume()\n",
            "            advance()\n",
            "        end\n",
            "    end)\n",
            ")\n",
            "outer(\n",
            "    inner(function(): boolean\n",
            "        if ready and active then\n",
            "            consume()\n",
            "            return true\n",
            "        end\n",
            "        return false\n",
            "    end)\n",
            ")\n",
        );

        let expected = concat!(
            "outer(inner(function()\n",
            "    if ready and active then\n",
            "        consume()\n",
            "        advance()\n",
            "    end\n",
            "end))\n",
            "outer(inner(function(): boolean\n",
            "    if ready and active then\n",
            "        consume()\n",
            "        return true\n",
            "    end\n",
            "    return false\n",
            "end))\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, expected);
        assert_eq!(source(&output, &options).unwrap(), output);

        options.calls.wrap = Wrap::Preserve;
        assert_eq!(source(input, &options).unwrap(), input);
    }

    #[test]
    fn wraps_nested_callbacks_only_past_the_opening_line_width() {
        let opening = "    outer(inner(function(): boolean";
        let input = format!("function run()\n{opening}\n        return true\n    end))\nend\n");

        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: opening.len(),
            ..FormatOptions::default()
        };

        assert_eq!(source(&input, &options).unwrap(), input);

        options.width -= 1;
        let output = source(&input, &options).unwrap();

        assert_eq!(
            output,
            concat!(
                "function run()\n",
                "    outer(\n",
                "        inner(function(): boolean\n",
                "            return true\n",
                "        end)\n",
                "    )\n",
                "end\n",
            )
        );

        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn preserves_keyed_table_rows_without_wrapping_short_calls() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local checks = {\n",
            "    plain = typeof(api.plain) == \"function\",\n",
            "    [\"api.alpha\"] = typeof(api.alpha) == \"function\",\n",
            "    [\"api.beta\"] = typeof(api.beta) == \"function\",\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_numeric_for_header_on_one_line_inside_a_table_function() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local handlers = {\n",
            "    invoke = function(items)\n",
            "        for i = #items, 1, -1 do\n",
            "            if items[i]() then\n",
            "                break\n",
            "            end\n",
            "        end\n",
            "    end,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_parenthesized_casts_and_unary_conditions_spaced() {
        let options = FormatOptions::default();

        let input = concat!(
            "local value = (factory :: Getter<number>)()\n",
            "local count = if #items > 0 then #items else 0\n",
            "local end_index = if offset == nil then #items else offset - 1\n",
            "if #items >= LIMIT then\n",
            "\ttable.remove(items, 1)\n",
            "end\n",
            "return (value :: Getter<number>)()\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_generic_type_arguments_inline_inside_wrapped_type_table() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "export type Registry = {\n",
            "    read first: (self: Registry) -> Outcome<string, number>,\n",
            "    read second: <T>(self: Registry, key: string, fallback: T) -> Outcome<T, string>,\n",
            "    read third: (self: Registry) -> Outcome<string, number>,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_multiline_table_operands_with_their_type_operators() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let intersection = concat!(
            "export type Combined = Base & {\n",
            "    read kind: \"READY\",\n",
            "    read changed: ((boolean) -> ())?,\n",
            "    read reset: (() -> ())?,\n",
            "}\n",
        );

        let union = concat!(
            "type Variant = Base | {\n",
            "    read status: boolean,\n",
            "} | string\n",
        );

        for input in [intersection, union] {
            let output = source(input, &options).unwrap();
            assert_eq!(output, input);
            assert_eq!(source(&output, &options).unwrap(), output);
        }
    }

    #[test]
    fn wraps_flat_type_unions_only_past_the_formatted_line_width() {
        let input = "type Variant = Outcome<number> | Missing | false\n";

        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: input.trim_end().len(),
            ..FormatOptions::default()
        };

        let inline = source(input, &options).unwrap();
        assert_eq!(inline, input);
        assert_eq!(source(&inline, &options).unwrap(), inline);

        options.width -= 1;
        let wrapped = source(input, &options).unwrap();

        assert_eq!(
            wrapped,
            "type Variant = Outcome<number>\n    | Missing\n    | false\n"
        );

        assert_eq!(source(&wrapped, &options).unwrap(), wrapped);
    }

    #[test]
    fn expands_entire_over_width_type_union() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let choices = (0..40)
            .map(|index| format!("\"choice_{index:02}\""))
            .collect::<Vec<_>>();

        let input = format!("export type Choices = {}\n", choices.join(" | "));
        let expected = format!("export type Choices = {}\n", choices.join("\n    | "));
        let output = source(&input, &options).unwrap();

        assert_eq!(output, expected);
        assert_eq!(source(&output, &options).unwrap(), output);

        assert_eq!(
            source("type Short = \"a\" | \"b\"\n", &options).unwrap(),
            "type Short = \"a\" | \"b\"\n"
        );
    }

    #[test]
    fn auto_wraps_type_and_value_tables_with_trailing_separators() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.tables.wrap = Wrap::Auto;
        options.types.table_wrap = Wrap::Auto;

        let value = "local settings = { active = true, }\n";
        let expanded_value = "local settings = {\n    active = true,\n}\n";
        assert_eq!(source(value, &options).unwrap(), expanded_value);
        assert_eq!(source(expanded_value, &options).unwrap(), expanded_value);

        let ty = "type Shape = { active: boolean, }\n";
        let expanded_type = "type Shape = {\n    active: boolean,\n}\n";
        assert_eq!(source(ty, &options).unwrap(), expanded_type);
        assert_eq!(source(expanded_type, &options).unwrap(), expanded_type);

        options.tables.trailing_comma = TrailingComma::Never;
        let without_comma = "local settings = {\n    active = true\n}\n";
        assert_eq!(source(value, &options).unwrap(), without_comma);
        assert_eq!(source(without_comma, &options).unwrap(), without_comma);

        options.types.table_separator = crate::config::TypeTableSeparator::Semicolon;
        let with_semicolon = "type Shape = {\n    active: boolean;\n}\n";
        assert_eq!(source(ty, &options).unwrap(), with_semicolon);
        assert_eq!(source(with_semicolon, &options).unwrap(), with_semicolon);
    }

    #[test]
    fn configures_interpolation_padding_independently_of_table_spacing() {
        let mut options = FormatOptions::default();

        let input = concat!(
            "local text = `sum={left+right}, grouped={(left + right)}, count={count}`\n",
            "local card = `value={ { enabled=true } }`\n",
        );

        let compact = concat!(
            "local text = `sum={left + right}, grouped={(left + right)}, count={count}`\n",
            "local card = `value={ { enabled = true }}`\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, compact);
        assert_eq!(source(&output, &options).unwrap(), output);

        options.spacing.braces = false;

        assert_eq!(
            source("local text = `value={count}`\n", &options).unwrap(),
            "local text = `value={count}`\n"
        );

        options.spacing.braces = true;
        options.spacing.interpolation = true;

        let padded = concat!(
            "local text = `sum={ left + right }, grouped={ (left + right) }, count={ count }`\n",
            "local card = `value={ { enabled = true } }`\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, padded);
        assert_eq!(source(&output, &options).unwrap(), output);

        options.spacing.braces = false;

        assert_eq!(
            source("local card = `value={ { enabled=true } }`\n", &options).unwrap(),
            "local card = `value={ {enabled = true} }`\n"
        );
    }

    #[test]
    fn keeps_interpolated_chains_intact_past_line_width() {
        let options = FormatOptions {
            width: 80,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local text = `alpha={first:read():format()}, beta={second:read()}, ",
            "gamma={third:read()}, delta={fourth:read()}, epsilon={fifth:read()}`\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_typeof_parentheses_tight_inside_generic_types() {
        let mut options = FormatOptions::default();
        let input = "type Value = wrapper.infer<typeof (ROOT)>\ntype Direct = typeof (ROOT)\n";
        let expected = "type Value = wrapper.infer<typeof(ROOT)>\ntype Direct = typeof(ROOT)\n";

        let output = source(input, &options).unwrap();
        assert_eq!(output, expected);
        assert_eq!(source(&output, &options).unwrap(), output);

        options.spacing.before_function_parentheses = BeforeFunctionParentheses::Always;
        assert_eq!(source(input, &options).unwrap(), expected);
    }

    #[test]
    fn formats_declarations_without_inventing_function_bodies() {
        let options = FormatOptions::default();

        let input =
            "declare function first( x:number ):number\n\ndeclare function second():number\n";

        let output = source(input, &options).unwrap();

        assert_eq!(
            output,
            "declare function first(x: number): number\n\ndeclare function second(): number\n"
        );

        assert_eq!(source(&output, &options).unwrap(), output);

        let class = "declare extern type Widget with\nvalue:string\nfunction get():string\nend\n\ndeclare function third():Widget\n";
        let formatted = source(class, &options).unwrap();

        assert_eq!(
            formatted,
            "declare extern type Widget with\n\tvalue: string\n\tfunction get(): string\nend\n\ndeclare function third(): Widget\n"
        );

        assert_eq!(source(&formatted, &options).unwrap(), formatted);
    }
}
