use clap::Args;
use instar_core::build::{self, Plan, Session};
use std::{io, path::PathBuf, process::ExitCode, thread, time::Duration};

#[derive(Args)]
pub struct Build {
    #[arg(help = "Project directory or instar.toml path")]
    path: Option<PathBuf>,

    #[arg(long, help = "Named build profile")]
    profile: Option<String>,

    #[arg(
        long,
        conflicts_with = "watch",
        help = "Print the build plan as JSON without publishing outputs"
    )]
    plan: bool,

    #[arg(
        long,
        help = "Rebuild changed inputs while retaining the last successful output on errors"
    )]
    watch: bool,
}

impl Build {
    pub fn run(self) -> io::Result<ExitCode> {
        let from = self.path.unwrap_or(std::env::current_dir()?);
        let mut session = Session::default();

        if self.plan {
            let plan = session.plan(&from, self.profile.as_deref())?;
            println!("{}", plan.json()?);

            return Ok(if plan.has_errors() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            });
        }

        if !self.watch {
            let plan = session.plan(&from, self.profile.as_deref())?;

            return publish(&plan).map(|success| {
                if success {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            });
        }

        let mut previous = None;
        let mut dependencies = Vec::new();
        let mut failure = String::new();

        loop {
            match build::observe(&from, self.profile.as_deref(), &dependencies) {
                Ok(observed) => {
                    if previous.as_ref() != Some(&observed) {
                        previous = Some(observed);

                        match session
                            .plan(&from, self.profile.as_deref())
                            .and_then(|plan| {
                                let success = publish(&plan)?;

                                Ok((plan, success))
                            }) {
                            Ok((plan, success)) => {
                                dependencies = plan.inputs();

                                if !success {
                                    eprintln!(
                                        "build output is stale; the last successful output is retained"
                                    );
                                }

                                failure.clear();
                            }

                            Err(error) => {
                                if error.kind() == io::ErrorKind::AlreadyExists
                                    || error.kind() == io::ErrorKind::PermissionDenied
                                    || error.kind() == io::ErrorKind::WouldBlock
                                {
                                    return Err(error);
                                }

                                eprintln!("build output is stale: {error}");
                                failure = error.to_string();
                            }
                        }
                    }
                }

                Err(error) => {
                    if error.kind() == io::ErrorKind::AlreadyExists
                        || error.kind() == io::ErrorKind::PermissionDenied
                        || error.kind() == io::ErrorKind::WouldBlock
                    {
                        return Err(error);
                    }

                    let message = error.to_string();

                    if failure != message {
                        eprintln!("build output is stale: {message}");
                        failure = message;
                    }

                    previous = None;
                }
            }

            thread::sleep(Duration::from_millis(150));
        }
    }
}

fn publish(plan: &Plan) -> io::Result<bool> {
    for diagnostic in &plan.diagnostics {
        eprintln!(
            "{}:{}..{}: {}: {}",
            diagnostic.path.display(),
            diagnostic.start,
            diagnostic.end,
            if diagnostic.error { "error" } else { "warning" },
            diagnostic.message
        );
    }

    if plan.has_errors() {
        return Ok(false);
    }

    let outcome = plan.publish()?;

    eprintln!(
        "build: {} written, {} unchanged, {} removed",
        outcome.written, outcome.unchanged, outcome.removed
    );

    Ok(true)
}
