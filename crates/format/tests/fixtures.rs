//! Formatter fixture syntax and lossless emission contracts.
use std::{error::Error, fs, io, num::NonZeroUsize, path::Path, time::Duration};

use instar_analysis::{Completion, Options, Reason};
use instar_format::configuration::{Configuration, IndentStyle, LineEnding};

use vermis::{
    emitter::{Emitter, LineEnding as EmittedLineEnding},
    token::TokenKind,
    tree::Tree,
};

#[test]
fn fixtures() -> Result<(), Box<dyn Error>> {
    let cases = glob::glob(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/*/*"))?
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(cases.len(), 48, "all formatter fixtures must be discovered");

    for case in cases {
        let configuration = match fs::read_to_string(case.join("configuration.toml")) {
            Ok(source) => toml::from_str::<Configuration>(&source)
                .map_err(|error| format!("{}: {error}", case.display()))?,

            Err(error) if error.kind() == io::ErrorKind::NotFound => Configuration::default(),
            Err(error) => return Err(error.into()),
        };

        configuration
            .validate()
            .map_err(|error| format!("{}: {error}", case.display()))?;

        let line_ending = match configuration.line_ending {
            LineEnding::Lf => EmittedLineEnding::LineFeed,
            LineEnding::Crlf => EmittedLineEnding::CarriageReturnLineFeed,
        };

        let input = fs::read(case.join("input.luau"))?;
        let expected = fs::read(case.join("expected.luau"))?;

        let actual = instar_format::format(
            &input,
            &configuration,
            &Options::new(Duration::from_secs(5)),
        )
        .map_err(|error| format!("{}: {error}", case.display()))?;

        assert_eq!(actual.completion, Completion::Complete);
        assert_eq!(actual.diagnostics, Vec::new());
        let actual = actual.output.expect("complete formatted output");

        assert_eq!(
            actual,
            expected,
            "{}: formatted input must match expected output",
            case.display()
        );

        let repeated = instar_format::format(
            &expected,
            &configuration,
            &Options::new(Duration::from_secs(5)),
        )
        .map_err(|error| format!("{}: {error}", case.display()))?;

        assert_eq!(repeated.completion, Completion::Complete);
        assert_eq!(repeated.diagnostics, Vec::new());

        assert_eq!(
            repeated.output.expect("idempotent output"),
            expected,
            "{}: expected output must be idempotent",
            case.display()
        );

        verify(&input, &expected, &actual, line_ending, &case);
    }

    Ok(())
}

fn verify(
    input: &[u8],
    expected: &[u8],
    actual: &[u8],
    line_ending: EmittedLineEnding,
    case: &Path,
) {
    let input = vermis::parse(input);
    let expected = vermis::parse(expected);
    let actual = vermis::parse(actual);

    for (name, tree) in [
        ("input.luau", &input),
        ("expected.luau", &expected),
        ("actual.luau", &actual),
    ] {
        assert!(
            tree.diagnostics.is_empty(),
            "{}: {:?}",
            case.join(name).display(),
            tree.diagnostics
        );

        let mut emitter = Emitter::from_tree(tree, line_ending);
        emitter.node(tree, tree.root);

        assert_eq!(
            emitter.finish().as_ref(),
            tree.source,
            "{}: emission must be lossless",
            case.join(name).display()
        );
    }

    assert_eq!(
        preserved(&input),
        preserved(&expected),
        "{}: comments and literal spelling must survive formatting",
        case.display()
    );

    assert_eq!(
        preserved(&input),
        preserved(&actual),
        "{}: formatting must preserve comments and literal spelling",
        case.display()
    );
}

fn preserved<'source>(tree: &Tree<'source>) -> Vec<&'source [u8]> {
    let mut tokens = tree
        .tokens
        .iter()
        .filter(|token| {
            matches!(
                token.kind,
                TokenKind::Comment
                    | TokenKind::BlockComment
                    | TokenKind::Number
                    | TokenKind::RawString
                    | TokenKind::InterpolatedStringStart
                    | TokenKind::InterpolatedStringMiddle
                    | TokenKind::InterpolatedStringEnd
                    | TokenKind::InterpolatedStringSimple
            )
        })
        .map(|token| token.bytes(tree.source))
        .collect::<Vec<_>>();

    tokens.sort_unstable();

    tokens
}

#[test]
fn require_ordering_respects_lexical_identity() -> Result<(), Box<dyn Error>> {
    let configuration: Configuration = toml::from_str("[requires]\norder='alphabetical'")?;

    for source in [
        "local require = function(value) return value end\nlocal z = require('./z')\nlocal a = require('./a')\n",
        "local function load(require)\nlocal z = require('./z')\nlocal a = require('./a')\nend\n",
        "require = custom\nlocal z = require('./z')\nlocal a = require('./a')\n",
        "local z = require('./z')\nlocal require = require('./a')\n",
        "local value = require('./z')\nlocal value = require('./a')\n",
        "local z = require('./z')\nlocal a: typeof(z) = require('./a')\n",
    ] {
        let output = String::from_utf8(
            instar_format::format(
                source.as_bytes(),
                &configuration,
                &Options::new(Duration::from_secs(5)),
            )?
            .output
            .expect("formatted source"),
        )?;

        assert!(
            output.find("./z").expect("first call") < output.find("./a").expect("second call"),
            "{output}"
        );
    }

    Ok(())
}

#[test]
fn failures_and_limits_never_return_partial_output() -> Result<(), Box<dyn Error>> {
    let configuration = Configuration::default();
    let source = b"local =";

    let failed = instar_format::format(
        source,
        &configuration,
        &Options::new(Duration::from_secs(5)),
    )?;

    let parsed = vermis::parse(source);
    assert_eq!(failed.completion, Completion::Complete);
    assert_eq!(failed.output, None);

    assert_eq!(
        failed.diagnostics,
        parsed
            .diagnostics
            .iter()
            .map(|diagnostic| instar_format::Diagnostic {
                range: [diagnostic.span.start, diagnostic.span.end],
                message: diagnostic.message.to_owned(),
            })
            .collect::<Vec<_>>()
    );

    assert_ne!(failed.diagnostics, Vec::new());
    let cancelled = Options::new(Duration::from_secs(5));
    cancelled.cancellation.cancel();

    for (options, reason) in [
        (cancelled, Reason::Cancelled),
        (Options::new(Duration::ZERO), Reason::Timeout),
    ] {
        let result = instar_format::format(b"return 1", &configuration, &options)?;
        assert_eq!(result.completion, Completion::Incomplete(reason));
        assert_eq!(result.output, None);
        assert_eq!(result.diagnostics, Vec::new());
    }

    Ok(())
}

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

    verify(
        source.as_bytes(),
        &output,
        &output,
        EmittedLineEnding::LineFeed,
        Path::new("layout"),
    );

    let repeated = instar_format::format(
        &output,
        &configuration,
        &Options::new(Duration::from_secs(5)),
    )?;

    assert_eq!(repeated.completion, Completion::Complete);
    assert_eq!(repeated.diagnostics, Vec::new());
    assert_eq!(repeated.output, Some(output.clone()));

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
