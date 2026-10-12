use std::{
    collections::{BTreeMap, BTreeSet},
    io, mem,
    sync::Arc,
    time::{Duration, Instant},
};

use cxx::UniquePtr;

use instar_shared::{
    EventPayload, SourceOutcome, SourceSnapshot,
    process::{self, Emitter},
};

use super::{
    operation::NativeOperation,
    protocol::{
        Operation, PackedAnalysisResult, Request, Response, ResponseOutcome, WireFactsInput,
        WireLintInput, WirePositionInput, WireReference,
    },
};

use crate::{
    boundary,
    conversion::{self, Captured},
    flags::{self, LanguageProfile},
    native::{AnalysisIdentity, AnalysisPurpose, AnalysisRequest, SemanticAvailability},
};

struct Session {
    engine: UniquePtr<boundary::Engine>,
    profile: LanguageProfile,
    captured: Arc<Captured>,
    revision: u64,
    availability: BTreeMap<AnalysisIdentity, SemanticAvailability>,
    inferred: BTreeSet<AnalysisIdentity>,
}

impl Session {
    fn new(input: &AnalysisRequest, captured: Arc<Captured>) -> io::Result<Self> {
        flags::apply(&input.profile.flags)?;
        let engine = boundary::create_engine().map_err(io::Error::other)?;

        if engine.is_null() {
            return Err(io::Error::other("native engine allocation failed"));
        }

        Ok(Self {
            engine,
            profile: input.profile.clone(),
            captured,
            revision: 0,
            availability: BTreeMap::new(),
            inferred: BTreeSet::new(),
        })
    }

    fn validate(&self, reference: &WireReference, inference: bool) -> io::Result<()> {
        conversion::validate_reference(reference, &self.captured, self.revision)?;

        if inference && !self.inferred.contains(&reference.analysis) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "requested native inference is unavailable",
            ));
        }

        Ok(())
    }

    fn availability(&self, reference: &WireReference) -> io::Result<&SemanticAvailability> {
        self.availability
            .get(&reference.analysis)
            .ok_or_else(|| conversion::invalid("native analysis availability is missing"))
    }
}

/// Serves the isolated captured-analysis worker role.
///
/// # Errors
/// Returns startup agreement, framing, native allocation, callback and transport failures.
pub fn run_worker() -> io::Result<()> {
    let mut session: Option<Session> = None;
    let mut snapshots = Vec::new();

    process::serve(
        "analysis",
        |request: Request, emitter: &Emitter<Response>| {
            handle_request(request, emitter, &mut session, &mut snapshots)
        },
    )
}

fn handle_request(
    request: Request,
    emitter: &Emitter<Response>,
    session: &mut Option<Session>,
    snapshots: &mut Vec<SourceSnapshot>,
) -> io::Result<Response> {
    let started = Instant::now();
    let identities = emitter.identities();

    if (request.identity, request.generation) != identities {
        return Err(conversion::invalid(
            "native request identities disagree with their active frame",
        ));
    }

    let outcome = match request.operation {
        Operation::Capture { index, snapshot } => {
            if index == 0 {
                snapshots.clear();
            }

            if index != snapshots.len()
                || snapshots
                    .iter()
                    .any(|captured| captured.version.identity == snapshot.version.identity)
            {
                return Err(conversion::invalid(
                    "native snapshot capture is out of order or duplicated",
                ));
            }

            snapshot.validate()?;
            let source = snapshot.version;
            snapshots.push(snapshot);

            ResponseOutcome::Captured { index, source }
        }

        Operation::Analyze(input) => {
            let snapshots = mem::take(snapshots);
            let input = input.unpack(&snapshots)?;

            analyze_request(input, session, emitter, request.remaining_budget, started)?
        }

        operation => {
            if !snapshots.is_empty() {
                return Err(conversion::invalid("native snapshot capture is incomplete"));
            }

            query_request(
                &operation,
                session,
                emitter,
                request.remaining_budget,
                started,
            )?
        }
    };

    Ok(Response {
        identity: identities.0,
        generation: identities.1,
        outcome,
    })
}

fn analyze_request(
    input: AnalysisRequest,
    session: &mut Option<Session>,
    emitter: &Emitter<Response>,
    remaining_budget: Option<Duration>,
    started: Instant,
) -> io::Result<ResponseOutcome> {
    let events = emitter.clone();
    let emit = move |payload| events.emit(payload);
    let captured = Arc::new(Captured::analysis(Arc::new(input))?);

    let input = captured
        .analysis
        .as_ref()
        .ok_or_else(|| conversion::invalid("worker analysis input is missing"))?;

    if let Some(active) = session.as_ref()
        && active.profile != input.profile
    {
        return Err(conversion::invalid(
            "native worker immutable language profile changed",
        ));
    }

    if session.is_none() {
        *session = Some(Session::new(input, Arc::clone(&captured))?);
    }

    let active = session
        .as_mut()
        .ok_or_else(|| io::Error::other("native worker session is unavailable"))?;

    let operation = NativeOperation::new(Arc::clone(&captured), remaining_budget, started, emit)?;
    let input = conversion::analysis_input(&captured)?;

    let native_result = active
        .engine
        .pin_mut()
        .analyze(&input, &operation)
        .map_err(io::Error::other)?;

    let inferred = native_result
        .modules
        .iter()
        .filter(|module| module.inferred)
        .map(|module| module.name.clone())
        .collect::<BTreeSet<_>>();

    let result = conversion::analysis_result(native_result, &captured).map_err(io::Error::other)?;

    conversion::validate_analysis_result(
        &result,
        captured
            .analysis
            .as_ref()
            .ok_or_else(|| conversion::invalid("worker analysis input is missing"))?,
        &captured,
    )?;

    active.revision = result.frontend_revision;

    active.availability = result
        .modules
        .iter()
        .map(|module| (module.analysis, module.availability.clone()))
        .collect();

    active.inferred = result
        .modules
        .iter()
        .filter(|module| inferred.contains(&conversion::module_name(module.analysis.module)))
        .map(|module| module.analysis)
        .collect();

    active.captured = captured;

    for module in &result.modules {
        emitter.emit(EventPayload::SourceOutcome(SourceOutcome {
            source: module.parsed.input.source.version,
            completion: module.completion.clone(),
        }))?;
    }

    Ok(ResponseOutcome::Analyzed(PackedAnalysisResult::new(
        &result,
        &active.captured,
    )?))
}

fn query_request(
    operation: &Operation,
    session: &mut Option<Session>,
    emitter: &Emitter<Response>,
    remaining_budget: Option<Duration>,
    started: Instant,
) -> io::Result<ResponseOutcome> {
    let active = session.as_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "native analysis has not been established",
        )
    })?;

    let events = emitter.clone();

    let operation_context = NativeOperation::new(
        Arc::clone(&active.captured),
        remaining_budget,
        started,
        move |payload| events.emit(payload),
    )?;

    match operation {
        Operation::Facts(input) => facts_request(active, input, &operation_context),
        Operation::Lint(input) => lint_request(active, input, &operation_context, emitter),
        Operation::Inspect(input) => inspection_request(active, input, &operation_context),
        Operation::Complete(input) => completion_request(active, input, &operation_context),

        Operation::Capture { .. } | Operation::Analyze(_) => {
            Err(conversion::invalid("invalid native operation dispatch"))
        }
    }
}

fn facts_request(
    active: &mut Session,
    input: &WireFactsInput,
    operation_context: &NativeOperation,
) -> io::Result<ResponseOutcome> {
    active.validate(&input.analysis, false)?;
    let native_input = conversion::facts_input(input)?;

    let result = active
        .engine
        .pin_mut()
        .facts(&native_input, operation_context)
        .map_err(io::Error::other)?;

    let result = conversion::facts_result(
        &result,
        input,
        &active.captured,
        active.availability(&input.analysis)?,
    )?;

    conversion::validate_facts_result(&result, input, &active.captured)?;

    Ok(ResponseOutcome::Facts(result))
}

fn lint_request(
    active: &mut Session,
    input: &WireLintInput,
    operation_context: &NativeOperation,
    emitter: &Emitter<Response>,
) -> io::Result<ResponseOutcome> {
    active.validate(&input.analysis, false)?;
    let native_input = conversion::lint_input(input)?;

    let result = active
        .engine
        .pin_mut()
        .lint(&native_input, operation_context)
        .map_err(io::Error::other)?;

    let result =
        conversion::lint_result(result, input, &active.captured).map_err(io::Error::other)?;

    conversion::validate_lint_result(&result, input, &active.captured)?;

    for finding in &result.findings {
        emitter.emit(EventPayload::Diagnostic(finding.diagnostic.clone()))?;
    }

    Ok(ResponseOutcome::Linted(result))
}

fn inspection_request(
    active: &mut Session,
    input: &WirePositionInput,
    operation_context: &NativeOperation,
) -> io::Result<ResponseOutcome> {
    active.validate(&input.analysis, true)?;
    let native_input = conversion::position_input(input, &active.captured)?;

    let result = active
        .engine
        .pin_mut()
        .inspect(&native_input, operation_context)
        .map_err(io::Error::other)?;

    let result = conversion::inspection_result(
        result,
        input,
        &active.captured,
        active.availability(&input.analysis)?,
    )?;

    conversion::validate_inspection_result(&result, input, &active.captured)?;

    Ok(ResponseOutcome::Inspected(result))
}

fn completion_request(
    active: &mut Session,
    input: &WirePositionInput,
    operation_context: &NativeOperation,
) -> io::Result<ResponseOutcome> {
    active.validate(&input.analysis, true)?;

    if input.analysis.purpose != AnalysisPurpose::Editor {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "completion requires established editor analysis",
        ));
    }

    let native_input = conversion::position_input(input, &active.captured)?;

    let result = active
        .engine
        .pin_mut()
        .complete(&native_input, operation_context)
        .map_err(io::Error::other)?;

    let result = conversion::completions_result(result, input, &active.captured)?;
    conversion::validate_completions_result(&result, input, &active.captured)?;

    Ok(ResponseOutcome::Completed(result))
}
