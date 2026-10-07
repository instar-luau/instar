use std::io::{self, BufRead, Write};

use instar_analysis::Completion;

use crate::{
    Configuration, boundary,
    frontend::{Cancellation, Host, Link, LintResult, checking, completion},
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

        if let Completion::Incomplete(reason) = completion(result.completion)? {
            return Err(reason.error());
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
                    links.into_iter().map(Link::from).collect(),
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

                Ok(Response::Checked(checking(request.modules, result)?))
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

                Ok(Response::Linted(LintResult::from_native(
                    request.modules,
                    result,
                )?))
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

        let response = (|| {
            if session.is_none() {
                session = Some(Session::new(request.flags.clone())?);
            }

            session
                .as_mut()
                .ok_or_else(|| io::Error::other("native worker is unavailable"))?
                .execute(request)
        })()
        .map_err(instar_analysis::error::Failure::from);

        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}
