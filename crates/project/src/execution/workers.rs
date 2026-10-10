use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Condvar, Mutex, PoisonError, mpsc},
    thread,
    time::Duration,
};

use instar_format::{FormatInput, FormatResult, Formatter};

use instar_luau::{
    AnalysisReference, AnalysisRequest, AnalysisResult, Completions, ConfigurationEvaluator,
    ConfigurationInput, ConfigurationResult, FactsInput, FactsResult, InspectionResult,
    LanguageProfile, LintInput, LintResult, ModuleIdentity, Native, PositionInput,
    SemanticCategory, UpstreamWarningSettings,
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

    Facts {
        closure: AnalysisRequest,
        module: ModuleIdentity,
        categories: Vec<SemanticCategory>,
    },

    Warnings {
        closure: AnalysisRequest,
        module: ModuleIdentity,
        warnings: UpstreamWarningSettings,
    },

    Inspect {
        closure: AnalysisRequest,
        module: ModuleIdentity,
        offset: usize,
    },

    Complete {
        closure: AnalysisRequest,
        module: ModuleIdentity,
        offset: usize,
    },

    Format(FormatInput),
}

pub(crate) enum WorkOutput {
    Configuration(ConfigurationResult),
    Asset(DownloadResult),
    Analysis(AnalysisResult),
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

            Work::Analysis(input) => self.analyze(&input, options).map(WorkOutput::Analysis),

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

        self.native.analyze(input, options)
    }

    fn reference(
        &mut self,
        input: &AnalysisRequest,
        module: ModuleIdentity,
        options: &Options,
    ) -> io::Result<AnalysisReference> {
        let result = self.analyze(input, options)?;

        result
            .modules
            .into_iter()
            .find(|result| result.analysis.module == module)
            .and_then(|result| result.reference)
            .ok_or_else(|| Reason::UnavailableSemantics.error())
    }

    fn finish(&mut self) -> io::Result<()> {
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
        let count = work.len();

        if count == 0 {
            return Vec::new();
        }

        let admitted = self.permit.is_some();
        self.leave();
        let jobs = options.jobs.get().min(count);

        while self.lanes.len() < jobs {
            self.lanes.push(Mutex::new(Lane::default()));
        }

        let queue = Mutex::new(work.into_iter().enumerate().collect::<VecDeque<_>>());
        let (sender, receiver) = mpsc::channel();

        let results = thread::scope(|scope| {
            for lane in self.lanes.iter().take(jobs) {
                let queue = &queue;
                let sender = sender.clone();
                let budget = Arc::clone(&self.budget);

                scope.spawn(move || {
                    loop {
                        let work = queue
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .pop_front();

                        let Some((index, work)) = work else {
                            break;
                        };

                        let output = budget.acquire(options).and_then(|_permit| {
                            lane.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .execute(work, options)
                        });

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
                        Err(io::Error::other("project lane stopped without an outcome"))
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
}
