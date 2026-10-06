//! Formatter worker interruption and error boundary contracts.

use instar_analysis::{Completion, Options, Reason};
use instar_format::{Configuration, format};

use std::{
    io, thread,
    time::{Duration, Instant},
};

#[test]
fn interrupted_work_never_returns_output_and_allows_retry() -> io::Result<()> {
    let source = "local value = 1\n".repeat(500_000);
    let configuration = Configuration::default();
    let started = Instant::now();

    let result = format(
        source.as_bytes(),
        &configuration,
        &Options::new(Duration::from_millis(20)),
    )?;

    assert_eq!(result.completion, Completion::Incomplete(Reason::Timeout));
    assert_eq!(result.output, None);
    assert_eq!(result.diagnostics, Vec::new());
    assert!(started.elapsed() < Duration::from_secs(2));

    let options = Options::new(Duration::from_secs(5));
    let cancellation = options.cancellation.clone();

    let monitor = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        cancellation.cancel();
    });

    let started = Instant::now();
    let result = format(source.as_bytes(), &configuration, &options);
    monitor.join().expect("cancellation thread");
    let result = result?;
    assert_eq!(result.completion, Completion::Incomplete(Reason::Cancelled));
    assert_eq!(result.output, None);
    assert_eq!(result.diagnostics, Vec::new());
    assert!(started.elapsed() < Duration::from_secs(2));

    let result = format(
        b"return 1",
        &configuration,
        &Options::new(Duration::from_secs(5)),
    )?;

    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.output.as_deref(), Some(b"return 1\n".as_slice()));
    assert_eq!(result.diagnostics, Vec::new());

    Ok(())
}

#[test]
fn worker_preserves_configuration_error_category() {
    let configuration = Configuration {
        include: Some(vec!["[".to_owned()]),
        ..Configuration::default()
    };

    let error = format(
        b"return 1",
        &configuration,
        &Options::new(Duration::from_secs(5)),
    )
    .expect_err("invalid configuration");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}
