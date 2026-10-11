use std::{
    io,
    sync::{Arc, Condvar, Mutex, PoisonError, mpsc},
    thread,
    time::Duration,
};

use instar_format::{FormatInput, FormatResult, Formatter};

use instar_luau::{
    AnalysisReference, AnalysisRequest, AnalysisResult, ClosureIdentity, Completions,
    ConfigurationEvaluator, ConfigurationInput, ConfigurationResult, FactsInput, FactsResult,
    InspectionResult, LanguageProfile, LintInput, LintResult, ModuleIdentity, Native,
    PositionInput, SemanticCategory, UpstreamWarningSettings,
};

use instar_shared::{Options, Reason};

use crate::acquisition::{Acquisition, DownloadResult};

pub(crate) enum Work {
    Configuration(ConfigurationInput),

    Asset {
        selection: String,
        expected_digest: String,
    },

    Analysis(AnalysisRequest),

    CachedAnalysis {
        closure: ClosureIdentity,
        result: Arc<AnalysisResult>,
    },

    Facts {
        closure: Arc<AnalysisRequest>,
        module: ModuleIdentity,
        categories: Vec<SemanticCategory>,
    },

    Warnings {
        closure: Arc<AnalysisRequest>,
        module: ModuleIdentity,
        warnings: UpstreamWarningSettings,
    },

    Inspect {
        closure: Arc<AnalysisRequest>,
        module: ModuleIdentity,
        offset: usize,
    },

    Complete {
        closure: Arc<AnalysisRequest>,
        module: ModuleIdentity,
        offset: usize,
    },

    Format(FormatInput),
}

impl Work {
    fn closure(&self) -> Option<ClosureIdentity> {
        match self {
            Self::Analysis(input) => Some(input.closure),
            Self::CachedAnalysis { closure, .. } => Some(*closure),
            Self::Facts { closure, .. }
            | Self::Warnings { closure, .. }
            | Self::Inspect { closure, .. }
            | Self::Complete { closure, .. } => Some(closure.closure),
            Self::Configuration(_) | Self::Asset { .. } | Self::Format(_) => None,
        }
    }
}

struct Group {
    closure: Option<ClosureIdentity>,
    work: Vec<(usize, Work)>,
}

pub(crate) enum WorkOutput {
    Configuration(ConfigurationResult),
    Asset(DownloadResult),
    Analysis(Arc<AnalysisResult>),
    Facts(FactsResult),
    Warnings(LintResult),
    Inspection(InspectionResult),
    Completions(Completions),
    Format(FormatResult),
}

#[derive(Default)]
struct BudgetState {
    running: usize,
    limit: usize,
}

#[derive(Default)]
pub(super) struct Budget {
    state: Mutex<BudgetState>,
    available: Condvar,
}

impl Budget {
    pub(super) fn configure(&self, limit: usize) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        state.limit = limit;
        self.available.notify_all();
    }

    fn acquire(self: &Arc<Self>, options: &Options) -> io::Result<Permit> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        loop {
            options.check()?;

            if state.running < state.limit {
                state.running += 1;

                return Ok(Permit(Arc::clone(self)));
            }

            let interval = options
                .remaining()
                .map_or(Duration::from_millis(10), |remaining| {
                    remaining.min(Duration::from_millis(10))
                });

            state = self
                .available
                .wait_timeout(state, interval)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

struct Permit(Arc<Budget>);

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);

        state.running -= 1;
        self.0.available.notify_one();
    }
}

#[derive(Default)]
struct Lane {
    native: Native,
    configuration: ConfigurationEvaluator,
    formatter: Formatter,
    acquisition: Acquisition,
    profile: Option<LanguageProfile>,
    closure: Option<ClosureIdentity>,
}

impl Lane {
    fn execute(&mut self, work: Work, options: &Options) -> io::Result<WorkOutput> {
        options.check()?;

        match work {
            Work::Configuration(input) => self
                .configuration
                .evaluate(&input, options)
                .map(WorkOutput::Configuration),

            Work::Asset {
                selection,
                expected_digest,
            } => self
                .acquisition
                .acquire(&selection, &expected_digest, options)
                .map(WorkOutput::Asset),

            Work::Format(input) => self
                .formatter
                .format(&input, options)
                .map(WorkOutput::Format),

            Work::Analysis(input) => self
                .analyze(&input, options)
                .map(Arc::new)
                .map(WorkOutput::Analysis),

            Work::CachedAnalysis { result, .. } => Ok(WorkOutput::Analysis(result)),

            Work::Facts {
                closure,
                module,
                categories,
            } => {
                let analysis = self.reference(&closure, module, options)?;

                self.native
                    .facts(
                        &FactsInput {
                            analysis,
                            categories,
                        },
                        options,
                    )
                    .map(WorkOutput::Facts)
            }

            Work::Warnings {
                closure,
                module,
                warnings,
            } => {
                let analysis = self.reference(&closure, module, options)?;

                self.native
                    .lint(&LintInput { analysis, warnings }, options)
                    .map(WorkOutput::Warnings)
            }

            Work::Inspect {
                closure,
                module,
                offset,
            } => {
                let analysis = self.reference(&closure, module, options)?;

                self.native
                    .inspect(&PositionInput { analysis, offset }, options)
                    .map(WorkOutput::Inspection)
            }

            Work::Complete {
                closure,
                module,
                offset,
            } => {
                let analysis = self.reference(&closure, module, options)?;

                self.native
                    .complete(&PositionInput { analysis, offset }, options)
                    .map(WorkOutput::Completions)
            }
        }
    }

    fn analyze(
        &mut self,
        input: &AnalysisRequest,
        options: &Options,
    ) -> io::Result<AnalysisResult> {
        if self
            .profile
            .as_ref()
            .is_some_and(|profile| profile != &input.profile)
        {
            self.native.finish()?;
            self.native = Native::default();
        }

        self.profile = Some(input.profile.clone());

        let result = self.native.analyze(input, options)?;
        self.closure = Some(input.closure);

        Ok(result)
    }

    fn reference(
        &mut self,
        input: &AnalysisRequest,
        module: ModuleIdentity,
        options: &Options,
    ) -> io::Result<AnalysisReference> {
        if let Some(reference) = self.native.reference(input, module)? {
            return Ok(reference);
        }

        let result = self.analyze(input, options)?;

        result
            .modules
            .into_iter()
            .find(|result| result.analysis.module == module)
            .and_then(|result| result.reference)
            .ok_or_else(|| Reason::UnavailableSemantics.error())
    }

    fn finish(&mut self) -> io::Result<()> {
        self.closure = None;
        let analysis = self.native.finish();
        let configuration = self.configuration.finish();
        let formatting = self.formatter.finish();
        let acquisition = self.acquisition.finish();

        analysis.and(configuration).and(formatting).and(acquisition)
    }
}

pub(crate) struct Workers {
    lanes: Vec<Mutex<Lane>>,
    budget: Arc<Budget>,
    permit: Option<Permit>,
}

impl Workers {
    pub(super) fn new(budget: Arc<Budget>) -> Self {
        Self {
            lanes: Vec::new(),
            budget,
            permit: None,
        }
    }

    pub(crate) fn enter(&mut self, options: &Options) -> io::Result<()> {
        options.check()?;

        if self.permit.is_none() {
            self.permit = Some(self.budget.acquire(options)?);
        }

        Ok(())
    }

    pub(crate) fn leave(&mut self) {
        self.permit = None;
    }

    pub(crate) fn execute(&mut self, work: Work, options: &Options) -> io::Result<WorkOutput> {
        self.batch(vec![work], options)
            .pop()
            .ok_or_else(|| io::Error::other("worker returned no result"))?
    }

    pub(crate) fn batch(
        &mut self,
        work: Vec<Work>,
        options: &Options,
    ) -> Vec<io::Result<WorkOutput>> {
        self.batch_with(work, options, &|_, _| Vec::new())
            .into_iter()
            .map(|(result, _)| result)
            .collect()
    }

    pub(crate) fn batch_with(
        &mut self,
        work: Vec<Work>,
        options: &Options,
        following: &(impl Fn(usize, &WorkOutput) -> Vec<(ModuleIdentity, Work)> + Sync),
    ) -> Vec<(
        io::Result<WorkOutput>,
        Vec<(ModuleIdentity, io::Result<WorkOutput>)>,
    )> {
        let count = work.len();

        if count == 0 {
            return Vec::new();
        }

        let admitted = self.permit.is_some();
        self.leave();
        let assignments = self.distribute(work, options.jobs.get());
        let (sender, receiver) = mpsc::channel();

        let results = thread::scope(|scope| {
            for (index, group) in assignments {
                let lane = &self.lanes[index];
                let sender = sender.clone();
                let budget = Arc::clone(&self.budget);

                scope.spawn(move || {
                    for (index, work) in group.work {
                        let output = budget.acquire(options).map(|_permit| {
                            let mut lane = lane.lock().unwrap_or_else(PoisonError::into_inner);
                            let result = lane.execute(work, options);
                            let prerequisites = result.as_ref().map_or_else(
                                |_| Vec::new(),
                                |result| {
                                    following(index, result)
                                        .into_iter()
                                        .map(|(module, work)| (module, lane.execute(work, options)))
                                        .collect()
                                },
                            );

                            (result, prerequisites)
                        });
                        let output = output.unwrap_or_else(|error| (Err(error), Vec::new()));

                        if sender.send((index, output)).is_err() {
                            break;
                        }
                    }
                });
            }

            drop(sender);
            let mut output = (0..count).map(|_| None).collect::<Vec<_>>();

            for (index, result) in receiver {
                output[index] = Some(result);
            }

            output
                .into_iter()
                .map(|result| {
                    result.unwrap_or_else(|| {
                        (
                            Err(io::Error::other("project lane stopped without an outcome")),
                            Vec::new(),
                        )
                    })
                })
                .collect()
        });

        if admitted {
            drop(self.enter(options));
        }

        results
    }

    pub(crate) fn discard(&mut self) -> io::Result<()> {
        let mut result = Ok(());

        for lane in &self.lanes {
            let finished = lane.lock().unwrap_or_else(PoisonError::into_inner).finish();

            if result.is_ok() {
                result = finished;
            }
        }

        self.lanes.clear();

        result
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
        self.leave();

        self.discard()
    }

    fn distribute(&mut self, work: Vec<Work>, limit: usize) -> Vec<(usize, Group)> {
        let mut groups: Vec<Group> = Vec::new();

        for (index, work) in work.into_iter().enumerate() {
            let closure = work.closure();
            let existing = closure.and_then(|closure| {
                groups
                    .iter_mut()
                    .find(|group| group.closure == Some(closure))
            });

            if let Some(group) = existing {
                group.work.push((index, work));
            } else {
                groups.push(Group {
                    closure,
                    work: vec![(index, work)],
                });
            }
        }

        let jobs = limit.min(groups.len());

        while self.lanes.len() < jobs {
            self.lanes.push(Mutex::new(Lane::default()));
        }

        let closures = self
            .lanes
            .iter()
            .map(|lane| lane.lock().unwrap_or_else(PoisonError::into_inner).closure)
            .collect::<Vec<_>>();

        groups.sort_by_key(|group| group.closure.is_none() || !closures.contains(&group.closure));
        let mut lanes = (0..self.lanes.len()).collect::<Vec<_>>();

        lanes.sort_by_key(|index| {
            closures[*index].is_none()
                || !groups.iter().any(|group| group.closure == closures[*index])
        });

        let mut assignments = lanes
            .into_iter()
            .take(jobs)
            .map(|index| {
                (
                    index,
                    Group {
                        closure: None,
                        work: Vec::new(),
                    },
                )
            })
            .collect::<Vec<_>>();

        for group in groups {
            let preferred = assignments.iter().position(|(index, _)| {
                group.closure.is_some() && closures[*index] == group.closure
            });

            let index = preferred.unwrap_or_else(|| {
                assignments
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, (_, group))| group.work.len())
                    .map_or(0, |(index, _)| index)
            });

            assignments[index].1.work.extend(group.work);
        }

        assignments
    }
}
