//! Formatter fixture syntax and lossless emission contracts.

use std::{
    error::Error,
    fs, io,
    path::Path,
    str,
    time::{Duration, Instant},
};

use instar_format::{
    FormatInput, FormatOutcome, FormatScope, FormatSettings, Formatter, settings::LineEnding,
};

use instar_shared::{Completion, Options, SourceIdentity, SourceSnapshot, SourceVersion};
use instar_syntax::SyntaxSettings;

use instar_syntax::{
    emitter::{Emitter, LineEnding as EmittedLineEnding},
    token::TokenKind,
    tree::Tree,
};

fn request(source: &[u8], settings: &FormatSettings) -> io::Result<FormatInput> {
    Ok(FormatInput {
        source: SourceSnapshot::from_bytes(
            SourceVersion {
                identity: SourceIdentity::new(1),
                revision: 1,
            },
            source,
        )?,
        settings: settings.clone(),
        syntax: SyntaxSettings::default(),
        scope: FormatScope::Whole,
    })
}

fn budget(duration: Duration) -> Options {
    Options::new(Some(Instant::now() + duration))
}

#[test]
fn fixtures() -> Result<(), Box<dyn Error>> {
    let mut cases = Vec::new();

    for category in fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))? {
        for case in fs::read_dir(category?.path())? {
            let case = case?;

            if case.file_type()?.is_dir() {
                cases.push(case.path());
            }
        }
    }

    cases.sort();

    assert_eq!(cases.len(), 60, "all formatter fixtures must be discovered");

    for case in cases {
        let configuration = match fs::read_to_string(case.join("configuration.toml")) {
            Ok(source) => toml::from_str::<FormatSettings>(&source)
                .map_err(|error| format!("{}: {error}", case.display()))?,

            Err(error) if error.kind() == io::ErrorKind::NotFound => FormatSettings::default(),
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

        let mut formatter = Formatter::default();

        let actual = formatter
            .format(
                &request(&input, &configuration)?,
                &budget(Duration::from_secs(5)),
            )
            .map_err(|error| format!("{}: {error}", case.display()))?;

        assert_eq!(actual.completion, Completion::Complete);
        assert_eq!(actual.diagnostics, Vec::new());

        let FormatOutcome::Validated(actual) = actual.outcome else {
            panic!("complete formatted output");
        };

        assert!(actual.preservation().preserved());
        let actual = actual.output().as_bytes().to_vec();

        assert_eq!(
            actual,
            expected,
            "{}: formatted input must match expected output",
            case.display()
        );

        let repeated = formatter
            .format(
                &request(&expected, &configuration)?,
                &budget(Duration::from_secs(5)),
            )
            .map_err(|error| format!("{}: {error}", case.display()))?;

        assert_eq!(repeated.completion, Completion::Complete);
        assert_eq!(repeated.diagnostics, Vec::new());

        assert_eq!(
            match repeated.outcome {
                FormatOutcome::Validated(output) => output.output().as_bytes().to_vec(),
                FormatOutcome::Unavailable { .. } => panic!("idempotent output"),
            },
            expected,
            "{}: expected output must be idempotent",
            case.display()
        );

        verify(&input, &expected, &actual, line_ending, &case);

        if case
            .parent()
            .is_some_and(|parent| parent.ends_with("identity"))
        {
            let output = str::from_utf8(&actual)?;

            assert!(
                output.find("./z").expect("first call") < output.find("./a").expect("second call"),
                "{}: {output}",
                case.display()
            );
        }
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
    let version = SourceVersion {
        identity: SourceIdentity::new(1),
        revision: 1,
    };

    let input_source = SourceSnapshot::from_bytes(version, input).expect("UTF-8 fixture input");

    let expected_source =
        SourceSnapshot::from_bytes(version, expected).expect("UTF-8 fixture output");

    let actual_source =
        SourceSnapshot::from_bytes(version, actual).expect("UTF-8 formatted fixture");

    let input_parse = instar_syntax::parse(
        &input_source,
        &SyntaxSettings::default(),
        &Options::default(),
    );

    let expected_parse = instar_syntax::parse(
        &expected_source,
        &SyntaxSettings::default(),
        &Options::default(),
    );

    let actual_parse = instar_syntax::parse(
        &actual_source,
        &SyntaxSettings::default(),
        &Options::default(),
    );

    let input = input_parse.tree.syntax();
    let expected = expected_parse.tree.syntax();
    let actual = actual_parse.tree.syntax();

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
        preserved(input),
        preserved(expected),
        "{}: comments and literal spelling must survive formatting",
        case.display()
    );

    assert_eq!(
        preserved(input),
        preserved(actual),
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
fn failures_and_limits_never_return_partial_output() -> Result<(), Box<dyn Error>> {
    let configuration = FormatSettings::default();
    let source = b"local =";

    let mut formatter = Formatter::default();

    let input = request(source, &configuration)?;
    let failed = formatter.format(&input, &budget(Duration::from_secs(5)))?;
    let parsed = instar_syntax::parse(&input.source, &input.syntax, &Options::default());
    assert_eq!(failed.completion, Completion::Complete);
    assert!(matches!(failed.outcome, FormatOutcome::Unavailable { .. }));
    assert_eq!(failed.diagnostics, parsed.diagnostics);

    assert_ne!(failed.diagnostics, Vec::new());
    let cancelled = budget(Duration::from_secs(5));
    cancelled.cancellation.cancel();

    for (options, reason) in [
        (cancelled, Completion::Cancelled),
        (budget(Duration::ZERO), Completion::TimedOut),
    ] {
        let result = formatter.format(&request(b"return 1", &configuration)?, &options)?;
        assert_eq!(result.completion, reason);
        assert!(matches!(result.outcome, FormatOutcome::Unavailable { .. }));
        assert_eq!(result.diagnostics, Vec::new());
    }

    Ok(())
}
