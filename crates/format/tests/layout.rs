//! Display-column and multiline-argument layout contracts.

use instar_analysis::{Completion, Options};
use instar_format::{Configuration, configuration::IndentStyle};
use std::{io, num::NonZeroUsize, time::Duration};

fn formatted(source: &str, width: usize) -> io::Result<String> {
    let configuration = Configuration {
        width: NonZeroUsize::new(width).expect("nonzero width"),
        indent_style: IndentStyle::Spaces,
        ..Configuration::default()
    };

    let result = instar_format::format(
        source.as_bytes(),
        &configuration,
        &Options::new(Duration::from_secs(5)),
    )?;

    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.diagnostics, Vec::new());
    let output = result.output.expect("complete output");

    assert_eq!(
        instar_format::format(
            &output,
            &configuration,
            &Options::new(Duration::from_secs(5))
        )?
        .output,
        Some(output.clone())
    );

    Ok(String::from_utf8(output).expect("UTF-8 output"))
}

#[test]
fn unicode_and_tabs_use_display_columns() -> io::Result<()> {
    for text in ["e\u{301}".repeat(8), "界".repeat(4), "👨‍👩‍👧‍👦".repeat(4)] {
        let source = format!("return call(\"{text}\")\n");
        assert_eq!(formatted(&source, 24)?, source);
    }

    assert_eq!(
        formatted("return call(\"a\tb\")", 18)?,
        "return call(\n    \"a\tb\"\n)\n"
    );

    Ok(())
}

#[test]
fn multiline_arguments_wrap_surrounding_arguments() -> io::Result<()> {
    let source = "dispatch(\"a fairly long argument\", function(value)\nreturn value\nend, \"another long argument\")";
    let expected = "dispatch(\n    \"a fairly long argument\",\n    function(value)\n        return value\n    end,\n    \"another long argument\"\n)\n";
    assert_eq!(formatted(source, 36)?, expected);

    assert_eq!(
        formatted(
            "dispatch(\"head\", function(value)\nreturn value\nend, \"tail\")",
            60
        )?,
        "dispatch(\"head\", function(value)\n    return value\nend, \"tail\")\n"
    );

    Ok(())
}
