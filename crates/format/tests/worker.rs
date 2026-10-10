//! Formatter worker interruption and error boundary contracts.

use std::{
    io, thread,
    time::{Duration, Instant},
};

use instar_format::{
    FormatInput, FormatOutcome, FormatScope, FormatSettings, Formatter,
    settings::{CustomGroup, Group, Requires},
};

use instar_shared::{Completion, Options, SourceIdentity, SourceSnapshot, SourceVersion};
use instar_syntax::SyntaxSettings;

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
fn interrupted_work_never_returns_output_and_allows_retry() -> io::Result<()> {
    let mut formatter = Formatter::default();
    let source = "local value = 1\n".repeat(500_000);
    let configuration = FormatSettings::default();
    let started = Instant::now();

    let result = formatter.format(
        &request(source.as_bytes(), &configuration)?,
        &budget(Duration::from_millis(20)),
    )?;

    assert_eq!(result.completion, Completion::TimedOut);
    assert!(matches!(result.outcome, FormatOutcome::Unavailable { .. }));
    assert_eq!(result.diagnostics, Vec::new());
    assert!(started.elapsed() < Duration::from_secs(2));
    let options = budget(Duration::from_secs(5));
    let cancellation = options.cancellation.clone();

    let monitor = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        cancellation.cancel();
    });

    let started = Instant::now();
    let result = formatter.format(&request(source.as_bytes(), &configuration)?, &options);
    monitor.join().expect("cancellation thread");
    let result = result?;
    assert_eq!(result.completion, Completion::Cancelled);
    assert!(matches!(result.outcome, FormatOutcome::Unavailable { .. }));
    assert_eq!(result.diagnostics, Vec::new());
    assert!(started.elapsed() < Duration::from_secs(2));

    let result = formatter.format(
        &request(b"return 1", &configuration)?,
        &budget(Duration::from_secs(5)),
    )?;

    assert_eq!(result.completion, Completion::Complete);

    let FormatOutcome::Validated(output) = result.outcome else {
        panic!("complete output");
    };

    assert_eq!(output.output().as_bytes(), b"return 1\n");
    assert_eq!(result.diagnostics, Vec::new());

    Ok(())
}

#[test]
fn worker_preserves_configuration_error_category() {
    let configuration = FormatSettings {
        requires: Requires {
            groups: vec![Group::Custom(CustomGroup {
                name: "custom".to_owned(),
                patterns: vec!["[".to_owned()],
            })],
            ..Requires::default()
        },
        ..FormatSettings::default()
    };

    let error = Formatter::default()
        .format(
            &request(b"return 1", &configuration).expect("UTF-8 input"),
            &budget(Duration::from_secs(5)),
        )
        .expect_err("invalid configuration");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}
