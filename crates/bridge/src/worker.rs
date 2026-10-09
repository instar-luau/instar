use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    mem,
};

use instar_analysis::{Completion, error::Failure, process::Message};

use crate::{
    Configuration, boundary, flags,
    frontend::{
        Cancellation, Hierarchy, Host, Hover, Link, LintResult, Reference, Signatures, Suggestions,
        checking, completion, location,
    },
    protocol::{Operation, Request, Response},
};

struct Session {
    native: cxx::UniquePtr<boundary::NativeFrontend>,
    host: Host,
    flags: BTreeMap<String, flags::Value>,
}

impl Session {
    fn new(flags: BTreeMap<String, flags::Value>) -> io::Result<Self> {
        flags::apply(&flags)?;
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

    fn synchronize(&mut self, request: &mut Request) -> io::Result<()> {
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

        self.host.sources = mem::take(&mut request.sources);
        self.host.declarations = mem::take(&mut request.declarations);
        self.host.environments = mem::take(&mut request.environments);
        self.host.progress = request.progress;

        Ok(())
    }

    fn documentation(&mut self, request: &Request, symbol: &str) -> io::Result<Response> {
        self.checked(request)?;

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

    fn hover(&mut self, request: &Request, line: u32, column: u32) -> io::Result<Response> {
        let entry = self.semantic(request)?;

        let result = self
            .native
            .pin_mut()
            .hover(&self.host, entry, line, column)
            .map_err(io::Error::other)?;

        Ok(Response::Hover(result.found.then(|| Hover {
            location: location(result.location),
            text: result.text,
            documentation: (!result.documentation.is_empty()).then_some(result.documentation),
            deprecated: result.deprecated,
        })))
    }

    fn definition(
        &mut self,
        request: &Request,
        line: u32,
        column: u32,
        type_definition: bool,
    ) -> io::Result<Response> {
        let entry = self.semantic(request)?;

        let locations = self
            .native
            .pin_mut()
            .definition(&self.host, entry, line, column, type_definition)
            .map_err(io::Error::other)?;

        Ok(Response::Locations(
            locations.into_iter().map(location).collect(),
        ))
    }

    fn suggest(&mut self, request: &Request, line: u32, column: u32) -> io::Result<Response> {
        let entry = self.semantic(request)?;

        let suggestions = self
            .native
            .pin_mut()
            .suggest(&self.host, entry, line, column)
            .map_err(io::Error::other)?;

        Ok(Response::Suggestions(Suggestions::try_from(suggestions)?))
    }

    fn signatures(&mut self, request: &Request, line: u32, column: u32) -> io::Result<Response> {
        let entry = self.semantic(request)?;

        let result = self
            .native
            .pin_mut()
            .signatures(&self.host, entry, line, column)
            .map_err(io::Error::other)?;

        Ok(Response::Signatures(if result.found {
            Some(Signatures::try_from(result)?)
        } else {
            None
        }))
    }

    fn references(&mut self, request: &Request) -> io::Result<Response> {
        self.semantic(request)?;

        let references = self
            .native
            .pin_mut()
            .references(&self.host, &request.modules)
            .map_err(io::Error::other)?;

        Ok(Response::References(
            references
                .into_iter()
                .map(Reference::try_from)
                .collect::<io::Result<_>>()?,
        ))
    }

    fn hierarchy(&mut self, request: &Request, position: Option<[u32; 2]>) -> io::Result<Response> {
        let entry = self.semantic(request)?;

        let hierarchy = self
            .native
            .pin_mut()
            .hierarchy(
                &self.host,
                &request.modules,
                entry,
                position
                    .as_ref()
                    .map_or(&[][..], |position| position.as_slice()),
            )
            .map_err(io::Error::other)?;

        Ok(Response::Hierarchy(Hierarchy::try_from(hierarchy)?))
    }

    fn semantic<'request>(&mut self, request: &'request Request) -> io::Result<&'request str> {
        self.native.pin_mut().retain().map_err(io::Error::other)?;
        self.checked(request)?;

        request
            .entries
            .first()
            .map(String::as_str)
            .ok_or_else(|| io::Error::other("semantic entry is missing"))
    }

    fn execute(&mut self, mut request: Request) -> io::Result<Response> {
        if self.flags != request.flags {
            return Err(io::Error::other("native worker flag configuration changed"));
        }

        self.synchronize(&mut request)?;
        let cancellation = Cancellation(instar_analysis::Cancellation::default(), None);

        match &request.operation {
            Operation::Documentation(symbol) => self.documentation(&request, symbol),

            Operation::Hover { line, column } => self.hover(&request, *line, *column),
            Operation::Suggest { line, column } => self.suggest(&request, *line, *column),
            Operation::Signatures { line, column } => self.signatures(&request, *line, *column),
            Operation::References => self.references(&request),
            Operation::Hierarchy { position } => self.hierarchy(&request, *position),

            Operation::Definition {
                line,
                column,
                type_definition,
            } => self.definition(&request, *line, *column, *type_definition),

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

    fn checked(&mut self, request: &Request) -> io::Result<()> {
        let cancellation = Cancellation(instar_analysis::Cancellation::default(), None);

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

        Ok(())
    }
}

/// Runs the native worker protocol over standard input and output.
///
/// # Errors
/// Returns initialization, protocol, or transport failures.
pub fn run() -> io::Result<()> {
    let mut session = None;
    let input = io::stdin().lock();

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
        .map_err(Failure::from);

        let mut output = io::stdout().lock();

        serde_json::to_writer(&mut output, &Message::Response(response))?;

        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}
