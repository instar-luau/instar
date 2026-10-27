use std::{
    io::{self, Write},
    iter::repeat_n,
};

use owo_colors::Style;
use similar::{ChangeTag, InlineChangeMode, InlineChangeOptions, TextDiff};

use super::highlight::Source;

pub(super) fn write(
    output: &mut impl Write,
    name: &str,
    before: &str,
    after: &str,
    indentation_width: usize,
) -> io::Result<()> {
    let diff = TextDiff::from_lines(before, after);
    let mut unified = diff.unified_diff();
    unified.header(name, name);

    if !console::colors_enabled() {
        return write!(output, "{unified}");
    }

    let is_terminal = console::Term::stdout().is_term();
    let before = Source::new(before);
    let after = Source::new(after);
    let mut options = InlineChangeOptions::new();
    options.mode(InlineChangeMode::Chars);
    let mut has_header = false;

    for hunk in unified.iter_hunks() {
        if !has_header {
            writeln!(
                output,
                "{}",
                Style::new().red().style(format_args!("--- {name}"))
            )?;

            writeln!(
                output,
                "{}",
                Style::new().green().style(format_args!("+++ {name}"))
            )?;

            has_header = true;
        }

        writeln!(output, "{}", Style::new().cyan().style(hunk.header()))?;

        let changes = hunk
            .ops()
            .iter()
            .flat_map(|operation| diff.iter_inline_changes_with_options(operation, options));

        for change in changes {
            let row = background(change.tag(), false);
            let fills_row = is_terminal && change.tag() != ChangeTag::Equal;

            let gutter = if fills_row { row } else { Style::new() };

            let (source, index, style) = match change.tag() {
                ChangeTag::Delete => (&before, change.old_index(), gutter.red()),
                ChangeTag::Insert => (&after, change.new_index(), gutter.green()),
                ChangeTag::Equal => (&after, change.new_index(), gutter),
            };

            let index = index.expect("Internal error: diff line has no source index.");

            if fills_row {
                fill(output, row)?;
            }

            write!(output, "{}", style.style(change.tag()))?;

            let mut indentation_length = 0;
            let mut has_changed_indentation = false;

            for (emphasized, value) in change.values() {
                let length = value
                    .bytes()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count();

                indentation_length += length;
                has_changed_indentation |= *emphasized && length != 0;

                if length != value.len() {
                    break;
                }
            }

            let mut offset = 0;
            let mut column = 0;

            for (emphasized, value) in change.values() {
                let content = value.trim_end_matches(['\r', '\n']);
                let boundary = indentation_length.saturating_sub(offset).min(content.len());

                for (start, end, emphasized) in [
                    (0, boundary, *emphasized || has_changed_indentation),
                    (boundary, content.len(), *emphasized),
                ] {
                    if start == end {
                        continue;
                    }

                    let background = background(change.tag(), emphasized);

                    for mut fragment in
                        source.span(index, offset + start, &content[start..end], background)
                    {
                        let expanded;

                        if is_terminal {
                            expanded = expand(fragment.inner(), &mut column, indentation_width);
                            *fragment.inner_mut() = &expanded;
                        }

                        write!(output, "{fragment}")?;
                    }
                }

                if !is_terminal {
                    write!(output, "{}", &value[content.len()..])?;
                }

                offset += value.len();
            }

            if fills_row {
                fill(output, row)?;
            }

            if is_terminal && !change.missing_newline() {
                writeln!(output)?;
            }

            if change.missing_newline() {
                writeln!(output, "\n\\ No newline at end of file")?;
            }
        }
    }

    Ok(())
}

fn expand(text: &str, column: &mut usize, width: usize) -> String {
    let mut expanded = String::new();

    for (index, part) in text.split('\t').enumerate() {
        if index != 0 {
            let padding = width - *column % width;
            expanded.push('→');
            expanded.extend(repeat_n(' ', padding - 1));
            *column += padding;
        }

        expanded.push_str(&part.replace(' ', "·"));
        *column += console::measure_text_width(part);
    }

    expanded
}

fn fill(output: &mut impl Write, style: Style) -> io::Result<()> {
    write!(
        output,
        "{}\u{1b}[K{}",
        style.prefix_formatter(),
        style.suffix_formatter(),
    )
}

fn background(tag: ChangeTag, emphasized: bool) -> Style {
    let style = Style::new().truecolor(230, 237, 243);

    match (tag, emphasized) {
        (ChangeTag::Delete, false) => style.on_truecolor(61, 28, 32),
        (ChangeTag::Delete, true) => style.on_truecolor(102, 39, 46),
        (ChangeTag::Insert, false) => style.on_truecolor(22, 49, 38),
        (ChangeTag::Insert, true) => style.on_truecolor(35, 83, 58),
        (ChangeTag::Equal, _) => Style::new(),
    }
}
