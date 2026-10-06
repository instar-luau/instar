//! Formatter fixture syntax and lossless emission contracts.

use std::{error::Error, fs, io};

use instar_format::configuration::{Configuration, LineEnding};

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

        let actual = instar_format::format(&input, &configuration)
            .map_err(|error| format!("{}: {error}", case.display()))?;

        assert_eq!(
            actual,
            expected,
            "{}: formatted input must match expected output",
            case.display()
        );

        assert_eq!(
            instar_format::format(&expected, &configuration)
                .map_err(|error| format!("{}: {error}", case.display()))?,
            expected,
            "{}: expected output must be idempotent",
            case.display()
        );

        let input = vermis::parse(&input);
        let expected = vermis::parse(&expected);
        let actual = vermis::parse(&actual);

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

    Ok(())
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
