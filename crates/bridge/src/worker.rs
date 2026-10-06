use std::io::{self, BufRead, Write};

use instar_analysis::{Completion, Diagnostic, Kind, Location, Related};

use crate::{
    Configuration, boundary,
    frontend::{Cancellation, Fact, FactKind, Host, Link, LintResult, Warning},
    protocol::{Operation, Request, Response},
};

struct Session {
    native: cxx::UniquePtr<boundary::NativeFrontend>,
    host: Host,
    flags: std::collections::BTreeMap<String, crate::flags::Value>,
}

impl Session {
    fn new(flags: std::collections::BTreeMap<String, crate::flags::Value>) -> io::Result<Self> {
        crate::flags::apply(&flags)?;
        let native = boundary::create_frontend().map_err(io::Error::other)?;

        if native.is_null() {
            return Err(io::Error::other("native frontend allocation failed"));
        }

        Ok(Self {
            native,
            host: Host::default(),
            flags,
        })
    }

    fn synchronize(&mut self, request: &Request) -> io::Result<()> {
        let removed = self
            .host
            .sources
            .keys()
            .filter(|name| !request.sources.contains_key(*name))
            .cloned()
            .collect::<Vec<_>>();

        self.native
            .pin_mut()
            .invalidate(&removed)
            .map_err(io::Error::other)?;

        for (name, source) in &request.sources {
            if self.host.sources.get(name).is_some_and(|previous| {
                previous.configuration == source.configuration
                    && previous.text == source.text
                    && previous.revision == source.revision
                    && previous.sites == source.sites
            }) {
                continue;
            }

            let configuration = Configuration::restore(&source.configuration)?;

            let configuration = configuration
                .native
                .as_ref()
                .ok_or_else(|| io::Error::other("native configuration is unavailable"))?;

            self.native
                .pin_mut()
                .configure(name, configuration)
                .map_err(io::Error::other)?;
        }

        self.host.sources = request.sources.clone();

        Ok(())
    }

    fn documentation(&mut self, request: &Request, symbol: &str) -> io::Result<Response> {
        let cancellation = Cancellation(instar_analysis::Cancellation::default());

        let result = self
            .native
            .pin_mut()
            .check(
                &self.host,
                &request.entries,
                request.timeout.as_secs_f64(),
                &request.modules,
                &cancellation,
            )
            .map_err(io::Error::other)?;

        if completion(result.completion)? != Completion::Complete || !result.diagnostics.is_empty()
        {
            return Err(io::Error::other(
                "documentation requires complete native analysis",
            ));
        }

        let entry = request
            .entries
            .first()
            .ok_or_else(|| io::Error::other("documentation entry is missing"))?;

        let symbol = self
            .native
            .documentation(entry, symbol)
            .map_err(io::Error::other)?;

        Ok(Response::Documentation(
            (!symbol.is_empty()).then_some(symbol),
        ))
    }

    fn execute(&mut self, request: Request) -> io::Result<Response> {
        if self.flags != request.flags {
            return Err(io::Error::other("native worker flag configuration changed"));
        }

        self.synchronize(&request)?;
        let cancellation = Cancellation(instar_analysis::Cancellation::default());

        match &request.operation {
            Operation::Documentation(symbol) => self.documentation(&request, symbol),

            Operation::Parse => {
                let links = self
                    .native
                    .pin_mut()
                    .prepare(&self.host, &request.modules)
                    .map_err(io::Error::other)?;

                Ok(Response::Parsed(
                    links
                        .into_iter()
                        .map(|link| Link {
                            module: link.module,
                            revision: link.revision,
                            call: [link.call_start, link.call_end],
                            argument: [link.argument_start, link.argument_end],
                            target: link.target,
                        })
                        .collect(),
                ))
            }

            Operation::Check => {
                let result = self
                    .native
                    .pin_mut()
                    .check(
                        &self.host,
                        &request.entries,
                        request.timeout.as_secs_f64(),
                        &request.modules,
                        &cancellation,
                    )
                    .map_err(io::Error::other)?;

                Ok(Response::Checked(instar_analysis::Result {
                    modules: request.modules,
                    diagnostics: result
                        .diagnostics
                        .into_iter()
                        .map(diagnostic)
                        .collect::<io::Result<_>>()?,
                    completion: completion(result.completion)?,
                }))
            }

            Operation::Lint(semantic) => {
                let result = self
                    .native
                    .pin_mut()
                    .lint(
                        &self.host,
                        &request.entries,
                        request.timeout.as_secs_f64(),
                        &request.modules,
                        &cancellation,
                        semantic,
                    )
                    .map_err(io::Error::other)?;

                Ok(Response::Linted(LintResult {
                    modules: request.modules,
                    warnings: result
                        .warnings
                        .into_iter()
                        .map(|warning| Warning {
                            location: location(warning.location),
                            code: warning.code,
                            name: warning.name,
                            message: warning.message,
                            fatal: warning.fatal,
                        })
                        .collect(),
                    facts: result
                        .facts
                        .into_iter()
                        .map(fact)
                        .collect::<io::Result<_>>()?,
                    diagnostics: result
                        .diagnostics
                        .into_iter()
                        .map(diagnostic)
                        .collect::<io::Result<_>>()?,
                    completion: completion(result.completion)?,
                }))
            }
        }
    }
}

/// Runs the native worker protocol over standard input and output.
///
/// # Errors
/// Returns initialization, protocol, or transport failures.
pub fn run() -> io::Result<()> {
    let mut session = None;
    let input = io::stdin().lock();
    let mut output = io::stdout().lock();

    for line in input.lines() {
        let request: Request = serde_json::from_str(&line?)?;

        if session.is_none() {
            session = Some(Session::new(request.flags.clone())?);
        }

        let response = session
            .as_mut()
            .ok_or_else(|| io::Error::other("native worker is unavailable"))?
            .execute(request)
            .map_err(|error| error.to_string());

        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}

fn completion(value: boundary::NativeCompletion) -> io::Result<Completion> {
    use instar_analysis::Reason;

    match value {
        boundary::NativeCompletion::Complete => Ok(Completion::Complete),
        boundary::NativeCompletion::Cancelled => Ok(Completion::Incomplete(Reason::Cancelled)),
        boundary::NativeCompletion::Timeout => Ok(Completion::Incomplete(Reason::Timeout)),
        boundary::NativeCompletion::Environment => Ok(Completion::Incomplete(Reason::Environment)),
        boundary::NativeCompletion::Analysis => Ok(Completion::Incomplete(Reason::Analysis)),
        _ => Err(io::Error::other("unknown native completion status")),
    }
}

fn fact(value: boundary::NativeFact) -> io::Result<Fact> {
    let kind = match value.kind {
        boundary::NativeFactKind::ImplicitAnyLocal => FactKind::ImplicitAnyLocal,
        boundary::NativeFactKind::ImplicitAnyParameter => FactKind::ImplicitAnyParameter,
        _ => return Err(io::Error::other("unknown native semantic fact kind")),
    };

    Ok(Fact {
        location: location(value.location),
        kind,
        message: value.message,
    })
}

fn location(value: boundary::NativeLocation) -> Location<String> {
    Location {
        module: value.module,
        revision: value.revision,
        range: [value.start, value.end],
    }
}

fn diagnostic(value: boundary::NativeDiagnostic) -> io::Result<Diagnostic<String>> {
    let kind = match value.kind {
        boundary::NativeKind::Syntax => Kind::Syntax {
            code: Some(value.code),
        },

        boundary::NativeKind::Type => Kind::Type { code: value.code },

        boundary::NativeKind::Resolution => Kind::Resolution {
            code: Some(value.code),
        },

        boundary::NativeKind::Analysis => Kind::Analysis {
            code: (value.code != 0).then_some(value.code),
        },

        _ => return Err(io::Error::other("unknown native diagnostic kind")),
    };

    Ok(Diagnostic {
        location: location(value.location),
        kind,
        message: value.message,
        related: value
            .related
            .into_iter()
            .map(|related| Related {
                location: location(related.location),
                message: related.message,
            })
            .collect(),
    })
}
