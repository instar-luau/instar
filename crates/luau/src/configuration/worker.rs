use std::{collections::BTreeMap, io, sync::Arc, time::Instant};

use instar_shared::{
    Completion, EventPayload, Reason,
    process::{self, Emitter},
};

use super::protocol::{ConfigurationRequest, ConfigurationResponse};

use crate::{
    boundary,
    conversion::{self, Captured},
    flags::{self, FlagValue},
    worker::operation::NativeOperation,
};

/// Serves request-local sandboxed configuration VMs in a reusable isolated worker.
///
/// # Errors
/// Returns startup agreement, framing, native callback and transport failures.
pub fn run_configuration_worker() -> io::Result<()> {
    let mut profile: Option<BTreeMap<String, FlagValue>> = None;

    process::serve(
        "configuration",
        |request: ConfigurationRequest, emitter: &Emitter<ConfigurationResponse>| {
            let started = Instant::now();
            let identities = emitter.identities();

            if (request.request, request.generation) != identities {
                return Err(conversion::invalid(
                    "configuration request identities disagree with their active frame",
                ));
            }

            conversion::validate_configuration(&request.input)?;
            let flags = flags::normalize(&request.input.language.flags)?;

            if profile.as_ref().is_some_and(|profile| profile != &flags) {
                return Err(conversion::invalid(
                    "configuration worker immutable language profile changed",
                ));
            }

            if profile.is_none() {
                flags::apply(&flags)?;
                profile = Some(flags);
            }

            let captured = Arc::new(Captured::configuration(&request.input.source)?);
            let events = emitter.clone();

            let operation = NativeOperation::new(
                captured,
                request.remaining_budget,
                started,
                move |payload| events.emit(payload),
            )?;

            let response = match conversion::configuration_input(&request.input) {
                Ok(input) => {
                    let output = boundary::evaluate_configuration(&input, &operation)
                        .map_err(io::Error::other)?;

                    conversion::configuration_response(output, &request.input, identities)
                        .map_err(io::Error::other)?
                }

                Err(error) => {
                    let completion = if error.kind() == io::ErrorKind::Other {
                        Completion::Incomplete(vec![Reason::AnalysisLimit])
                    } else {
                        Completion::Incomplete(vec![Reason::FailedPrerequisite])
                    };

                    let result = conversion::configuration_failure(
                        &request.input,
                        error.to_string(),
                        completion,
                    )?;

                    ConfigurationResponse {
                        request: identities.0,
                        generation: identities.1,
                        source: result.source,
                        root: None,
                        nodes: Vec::new(),
                        diagnostics: result.diagnostics,
                        completion: result.completion,
                    }
                }
            };

            for diagnostic in &response.diagnostics {
                emitter.emit(EventPayload::Diagnostic(diagnostic.clone()))?;
            }

            Ok(response)
        },
    )
}
