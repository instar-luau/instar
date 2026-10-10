use std::{collections::BTreeMap, io};

use instar_shared::{
    Completion, Options,
    process::{Outcome, Process},
};

use super::{
    evaluation::{ConfigurationInput, ConfigurationResult},
    protocol::{ConfigurationRequest, ConfigurationResponse},
};

use crate::{
    conversion,
    flags::{self, FlagValue},
};

/// A lazy configuration worker client with request-local VMs and immutable language flags.
#[derive(Default)]
pub struct ConfigurationEvaluator {
    process: Process<ConfigurationRequest, ConfigurationResponse>,
    profile: Option<BTreeMap<String, FlagValue>>,
}

impl ConfigurationEvaluator {
    /// Evaluates and validates generic configuration values from captured source only.
    /// Limits bound VM allocations and extraction, not compiler or total host allocations.
    ///
    /// # Errors
    /// Returns invalid input, profile, startup, native, framing or worker cleanup failures.
    pub fn evaluate(
        &mut self,
        input: &ConfigurationInput,
        options: &Options,
    ) -> io::Result<ConfigurationResult> {
        conversion::validate_configuration(input)?;
        let profile = flags::normalize(&input.language.flags)?;

        if self
            .profile
            .as_ref()
            .is_some_and(|previous| previous != &profile)
        {
            self.process.discard()?;
        }

        self.profile = Some(profile);
        let mut identities = None;

        let outcome = self.process.request_with(
            "configuration",
            options,
            |request, generation, remaining_budget| {
                identities = Some((request, generation));

                ConfigurationRequest {
                    request,
                    generation,
                    remaining_budget,
                    input: input.clone(),
                }
            },
        )?;

        let response = match outcome {
            Outcome::Response(response) => response,

            Outcome::Interrupted(completion) => {
                return Ok(ConfigurationResult {
                    source: input.source.version,
                    values: None,
                    diagnostics: Vec::new(),
                    completion,
                });
            }
        };

        if identities != Some((response.request, response.generation))
            || response.source != input.source.version
        {
            return self.reject(conversion::invalid(
                "configuration terminal response identities disagree",
            ));
        }

        if matches!(
            response.completion,
            Completion::Cancelled | Completion::TimedOut
        ) {
            self.process.discard()?;
        }

        match conversion::configuration_wire_result(&response, input) {
            Ok(result) => Ok(result),
            Err(error) => self.reject(error),
        }
    }

    /// Stops acceptance and terminates and reaps the configuration worker.
    ///
    /// # Errors
    /// Returns worker termination, reaping or transport cleanup failures.
    pub fn finish(&mut self) -> io::Result<()> {
        self.process.finish()
    }

    fn reject<Output>(&mut self, error: io::Error) -> io::Result<Output> {
        match self.process.discard() {
            Ok(()) => Err(error),

            Err(cleanup) => Err(io::Error::new(
                error.kind(),
                format!("{error}; worker cleanup failed: {cleanup}"),
            )),
        }
    }
}
