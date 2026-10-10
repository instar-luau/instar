use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Write},
    io, iter,
    sync::Arc,
};

use instar_shared::{
    Availability, Completion, Diagnostic, DiagnosticCategory, DiagnosticTag, FindingIdentity,
    Knowledge, LinePosition, Location, Reason, RelatedLocation, Severity, SourceSnapshot,
    SourceVersion, Span,
};

use serde_json::Value;

use crate::{
    boundary::native,
    configuration::{
        ConfigurationEntry, ConfigurationInput, ConfigurationKey, ConfigurationLimits,
        ConfigurationResult, ConfigurationValue,
        protocol::{ConfigurationResponse, Entry, Node},
    },
    flags::{self, FlagValue, LanguageProfile},
    native::{
        AnalysisIdentity, AnalysisPurpose, AnalysisRequest, ArgumentRelation, BindingFact,
        BindingKind, CallFact, CallableSignature, CompletionCandidate, CompletionContext,
        CompletionKind, Completions, ConstantValue, ControlFlow, Determinism, EffectFact,
        EnvironmentInput, EnvironmentMetadata, ExpressionFact, FactsResult, FunctionFact,
        ImportOutcome, ImportRequest, InferenceMode, InheritanceFact, InspectionResult,
        InstanceRoot, InstanceStep, LanguageIdentity, LanguageMode, LintResult, ModuleIdentity,
        ModuleInput, NominalFact, Notation, OccurrenceAccess, OccurrenceFact, OccurrenceRole,
        Parentheses, ParsedMetadata, PositionFacts, ResolvedImport, ScopeFact,
        SemanticAvailability, SemanticCategory, SemanticFacts, SemanticIdentity, SemanticKind,
        SignatureParameter, SourceDirective, TypeFact, UpstreamFinding, UpstreamWarningIdentity,
        WarningAvailability,
    },
    worker::protocol::{
        WireAnalysisResult, WireFactsInput, WireLintInput, WireModuleAnalysis, WirePositionInput,
        WireReference,
    },
};

pub(crate) struct Captured {
    pub analysis: Option<Arc<AnalysisRequest>>,
    pub documents: BTreeMap<String, SourceSnapshot>,
    pub lines: BTreeMap<String, Vec<usize>>,
    pub modules: BTreeMap<String, ModuleInput>,
}

struct FactIdentities {
    declarations: BTreeMap<Location, SemanticIdentity>,
    environment_bindings: BTreeMap<(String, SemanticKind), SemanticIdentity>,
}

struct PositionEvidence {
    identities: Vec<SemanticIdentity>,
    definitions: Vec<Location>,
    type_definitions: Vec<Location>,
    signature_information: Knowledge<Vec<CallableSignature>>,
}

impl Captured {
    pub(crate) fn analysis(input: Arc<AnalysisRequest>) -> io::Result<Self> {
        validate_request(&input)?;

        let mut captured = Self {
            analysis: None,
            documents: BTreeMap::new(),
            lines: BTreeMap::new(),
            modules: BTreeMap::new(),
        };

        for module in &input.modules {
            captured.insert(module_name(module.analysis.module), &module.source.source)?;

            if let Some(environment) = &module.environment {
                for group in &environment.declarations {
                    for definition in &group.definitions {
                        captured.insert(
                            declaration_name(definition.source.version),
                            &definition.source,
                        )?;
                    }
                }
            }

            captured
                .modules
                .insert(module_name(module.analysis.module), module.clone());
        }

        captured.analysis = Some(input);

        Ok(captured)
    }

    pub(crate) fn configuration(source: &SourceSnapshot) -> io::Result<Self> {
        let mut captured = Self {
            analysis: None,
            documents: BTreeMap::new(),
            lines: BTreeMap::new(),
            modules: BTreeMap::new(),
        };

        captured.insert(declaration_name(source.version), source)?;

        Ok(captured)
    }

    pub(crate) fn module(&self, identity: AnalysisIdentity) -> io::Result<&ModuleInput> {
        self.modules
            .get(&module_name(identity.module))
            .filter(|module| module.analysis == identity)
            .ok_or_else(|| invalid("logical analysis identity is not captured"))
    }

    pub(crate) fn document(&self, name: &str) -> io::Result<&SourceSnapshot> {
        self.documents
            .get(name)
            .ok_or_else(|| invalid("source is not captured"))
    }

    pub(crate) fn offset(&self, name: &str, position: LinePosition) -> io::Result<usize> {
        let source = self.document(name)?;

        let lines = self
            .lines
            .get(name)
            .ok_or_else(|| invalid("native source coordinate has no captured line index"))?;

        let start = *lines
            .get(position.line)
            .ok_or_else(|| invalid("native source line is outside the captured document"))?;

        let end = lines
            .get(position.line + 1)
            .map_or(source.text.len(), |next| next - 1);

        let offset = start
            .checked_add(position.column)
            .ok_or_else(|| invalid("native source column overflows"))?;

        if offset > end || !source.text.is_char_boundary(offset) {
            return Err(invalid(
                "native source column is outside a UTF-8 line boundary",
            ));
        }

        Ok(offset)
    }

    pub(crate) fn position(&self, name: &str, offset: usize) -> io::Result<LinePosition> {
        let source = self.document(name)?;

        Span {
            start: offset,
            end: offset,
        }
        .validate(source)?;

        let lines = self
            .lines
            .get(name)
            .ok_or_else(|| invalid("native source coordinate has no captured line index"))?;

        let line = lines.partition_point(|start| *start <= offset) - 1;

        Ok(LinePosition {
            line,
            column: offset - lines[line],
        })
    }

    pub(crate) fn location(&self, input: &native::NativeLocation) -> io::Result<Location> {
        let source = self.document(&input.module)?;

        if source.version.revision != input.revision {
            return Err(invalid(
                "native location has an inconsistent source revision",
            ));
        }

        Location::new(
            source,
            Span {
                start: input.start,
                end: input.end,
            },
        )
        .map_err(invalid)
    }

    pub(crate) fn validate_location(&self, location: Location) -> io::Result<()> {
        let source = self
            .documents
            .values()
            .find(|source| source.version == location.source)
            .ok_or_else(|| invalid("result source revision is not captured"))?;

        location.validate(source).map_err(invalid)
    }

    pub(crate) fn inputs(&self) -> Vec<SourceVersion> {
        self.documents
            .values()
            .map(|source| source.version)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn insert(&mut self, name: String, source: &SourceSnapshot) -> io::Result<()> {
        source.validate()?;

        if let Some(previous) = self
            .documents
            .values()
            .find(|previous| previous.version == source.version)
            && previous != source
        {
            return Err(invalid("captured source revision has conflicting bytes"));
        }

        if let Some(previous) = self.documents.insert(name.clone(), source.clone())
            && previous != *source
        {
            return Err(invalid("captured document identity is inconsistent"));
        }

        self.lines.insert(
            name,
            iter::once(0)
                .chain(
                    source
                        .text
                        .bytes()
                        .enumerate()
                        .filter_map(|(offset, byte)| (byte == b'\n').then_some(offset + 1)),
                )
                .collect(),
        );

        Ok(())
    }
}

pub(crate) fn module_name(identity: ModuleIdentity) -> String {
    format!(
        "module:{}:{}",
        identity.context.get(),
        identity.source.get()
    )
}

pub(crate) fn declaration_name(version: SourceVersion) -> String {
    format!(
        "declaration:{}:{}",
        version.identity.get(),
        version.revision
    )
}

pub(crate) fn invalid(message: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

pub(crate) fn validate_profile(profile: &LanguageProfile) -> io::Result<()> {
    if LanguageProfile::new(&profile.flags, profile.type_function_heap_bytes)? != *profile {
        return Err(invalid("language profile is not normalized"));
    }

    Ok(())
}

pub(crate) fn validate_request(input: &AnalysisRequest) -> io::Result<()> {
    validate_profile(&input.profile)?;

    if input.modules.is_empty() || input.roots.is_empty() {
        return Err(invalid("analysis requires supplied modules and roots"));
    }

    if input
        .modules
        .windows(2)
        .any(|pair| pair[0].analysis.module >= pair[1].analysis.module)
    {
        return Err(invalid(
            "captured modules must be uniquely and deterministically ordered",
        ));
    }

    let modules = input
        .modules
        .iter()
        .map(|module| (module.analysis.module, module))
        .collect::<BTreeMap<_, _>>();

    let mut roots = BTreeSet::new();

    for root in &input.roots {
        if !modules.contains_key(root) || !roots.insert(*root) {
            return Err(invalid("analysis root is missing or duplicated"));
        }
    }

    let mut environments = BTreeMap::new();
    let mut snapshots = BTreeMap::new();

    for module in &input.modules {
        let source = &module.source.source;
        source.validate()?;

        if module.analysis.module != module.source.module
            || module.analysis.module.source != source.version.identity
        {
            return Err(invalid(
                "module source and logical analysis identities disagree",
            ));
        }

        if source.text.len() > u32::MAX as usize {
            return Err(invalid("source exceeds native coordinate limits"));
        }

        validate_snapshot(&mut snapshots, source)?;
        validate_features(&module.source.language.flags, &input.profile)?;

        if module.reporting.ordinary_type_diagnostics && !module.inference.required {
            return Err(invalid("ordinary checking requires established inference"));
        }

        let mut retained = BTreeSet::new();

        for category in &module.inference.retained {
            if !module.inference.required || !retained.insert(*category) {
                return Err(invalid("invalid retained semantic categories"));
            }
        }

        if module.inference.required && module.environment.is_none() {
            return Err(invalid("inference requires a captured environment"));
        }

        if let Some(environment) = &module.environment {
            validate_environment(environment, &input.profile, &mut snapshots)?;

            if let Some(previous) = environments.insert(environment.identity, environment)
                && previous != environment
            {
                return Err(invalid("environment identity has conflicting inputs"));
            }
        }

        validate_imports(module, &modules)?;
    }

    for cycle in &input.cycles {
        if cycle.edges.is_empty() {
            return Err(invalid("captured cycle path is empty"));
        }

        for (index, edge) in cycle.edges.iter().enumerate() {
            let source = modules
                .get(&edge.from)
                .ok_or_else(|| invalid("cycle source is not captured"))?;

            edge.site.validate(&source.source.source)?;

            if cycle.edges[(index + 1) % cycle.edges.len()].from != edge.to
                || !source.imports.iter().any(|import| {
                    import.site.call == edge.site
                        && import.outcome == ImportOutcome::Module(edge.to)
                })
            {
                return Err(invalid("captured cycle path disagrees with static imports"));
            }
        }
    }

    Ok(())
}

fn validate_imports(
    module: &ModuleInput,
    modules: &BTreeMap<ModuleIdentity, &ModuleInput>,
) -> io::Result<()> {
    let source = &module.source.source;
    let mut sites = BTreeSet::new();
    let mut ranges = BTreeSet::new();

    for import in &module.imports {
        import.site.call.validate(source)?;
        import.site.argument.validate(source)?;

        if import.site.call.span.start == import.site.call.span.end
            || import.site.argument.span.start < import.site.call.span.start
            || import.site.argument.span.end > import.site.call.span.end
            || !sites.insert(import.site.identity)
            || !ranges.insert(import.site.call.span)
        {
            return Err(invalid(
                "captured import sites are duplicated or inconsistent",
            ));
        }

        match (&import.site.request, &import.outcome) {
            (
                ImportRequest::Nonconstant | ImportRequest::Instance(_),
                ImportOutcome::Nonconstant,
            )
            | (ImportRequest::Static(_) | ImportRequest::Instance(_), ImportOutcome::Unresolved) => {
            }

            (
                ImportRequest::Static(_) | ImportRequest::Instance(_),
                ImportOutcome::Module(target),
            ) if modules.contains_key(target) => {}

            _ => {
                return Err(invalid(
                    "captured import request and outcome disagree or omit a target",
                ));
            }
        }
    }

    Ok(())
}

fn validate_features(
    flags: &BTreeMap<String, FlagValue>,
    profile: &LanguageProfile,
) -> io::Result<()> {
    if LanguageProfile::new(flags, profile.type_function_heap_bytes)?.flags != profile.flags {
        return Err(invalid(
            "captured input disagrees with immutable language profile",
        ));
    }

    Ok(())
}

fn validate_snapshot<'source>(
    snapshots: &mut BTreeMap<SourceVersion, &'source SourceSnapshot>,
    source: &'source SourceSnapshot,
) -> io::Result<()> {
    source.validate()?;

    if let Some(previous) = snapshots.insert(source.version, source)
        && previous != source
    {
        return Err(invalid("captured revision has conflicting source bytes"));
    }

    if snapshots
        .values()
        .any(|previous| previous.version.identity == source.version.identity && *previous != source)
    {
        return Err(invalid(
            "captured source identity has conflicting snapshots",
        ));
    }

    Ok(())
}

fn validate_environment<'source>(
    environment: &'source EnvironmentInput,
    profile: &LanguageProfile,
    snapshots: &mut BTreeMap<SourceVersion, &'source SourceSnapshot>,
) -> io::Result<()> {
    validate_features(&environment.language.flags, profile)?;
    let mut globals = BTreeSet::new();

    for global in &environment.globals {
        if global.is_empty() || global.contains('\0') || !globals.insert(global) {
            return Err(invalid("captured globals are empty or duplicated"));
        }
    }

    let mut definitions = BTreeSet::new();

    for group in &environment.declarations {
        for definition in &group.definitions {
            validate_snapshot(snapshots, &definition.source)?;

            if !valid_namespace(&definition.namespace)
                || !definitions.insert((definition.source.version, &definition.namespace))
            {
                return Err(invalid(
                    "captured declarations have invalid or duplicate namespaces",
                ));
            }
        }
    }

    let mut classes = BTreeSet::new();

    for class in &environment.capabilities.classes {
        if class.name.is_empty() || !classes.insert(&class.name) {
            return Err(invalid("captured classes are empty or duplicated"));
        }

        let mut properties = BTreeSet::new();

        for property in &class.properties {
            if property.name.is_empty() || !properties.insert(&property.name) {
                return Err(invalid("captured class properties are empty or duplicated"));
            }
        }
    }

    if let Some(environment) = &environment.instances {
        let mut instances = BTreeMap::new();

        for instance in &environment.instances {
            if instance.identity.is_empty()
                || instance.class.is_empty()
                || instances.insert(&instance.identity, instance).is_some()
            {
                return Err(invalid(
                    "captured instance identities are empty or duplicated",
                ));
            }
        }

        if !instances.contains_key(&environment.root)
            || !instances.contains_key(&environment.script)
        {
            return Err(invalid("captured instance root or script is missing"));
        }

        for instance in &environment.instances {
            let mut children = BTreeSet::new();

            for child in &instance.children {
                if !children.insert(child)
                    || instances
                        .get(child)
                        .is_none_or(|child| child.parent.as_ref() != Some(&instance.identity))
                {
                    return Err(invalid("captured instance children disagree with parents"));
                }
            }

            if let Some(parent) = &instance.parent {
                if instances
                    .get(parent)
                    .is_none_or(|parent| !parent.children.contains(&instance.identity))
                {
                    return Err(invalid("captured instance parent omits its child"));
                }
            } else if instance.identity != environment.root {
                return Err(invalid("captured instance has no mapped root"));
            }

            let mut ancestors = BTreeSet::new();
            let mut current = instance;

            while let Some(parent) = &current.parent {
                if !ancestors.insert(&current.identity) {
                    return Err(invalid("captured instance parent cycle"));
                }

                current = instances
                    .get(parent)
                    .ok_or_else(|| invalid("captured instance parent is missing"))?;
            }

            if current.identity != environment.root {
                return Err(invalid("captured instance is disconnected from its root"));
            }
        }
    }

    Ok(())
}

fn valid_namespace(namespace: &str) -> bool {
    namespace.strip_prefix('@').is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

pub(crate) fn analysis_input(input: &AnalysisRequest) -> io::Result<native::NativeAnalysisRequest> {
    Ok(native::NativeAnalysisRequest {
        signature: serde_json::to_string(input).map_err(invalid)?,
        purpose: purpose(input.purpose),
        roots: input.roots.iter().copied().map(module_name).collect(),
        modules: input
            .modules
            .iter()
            .map(module_input)
            .collect::<io::Result<_>>()?,
    })
}

fn module_input(input: &ModuleInput) -> io::Result<native::NativeModuleInput> {
    let environment = input.environment.as_ref();

    Ok(native::NativeModuleInput {
        name: module_name(input.analysis.module),
        text: input.source.source.text.to_string(),
        revision: input.source.source.version.revision,
        logical_revision: input.analysis.revision,
        signature: serde_json::to_string(input).map_err(invalid)?,
        environment_signature: serde_json::to_string(&input.environment).map_err(invalid)?,
        declaration_syntax: input.source.language.declaration_syntax,
        inference: input.inference.required,
        mode: match input.inference.mode {
            InferenceMode::Strict => 0,
            InferenceMode::Nonstrict => 1,
        },
        reporting: input.reporting.ordinary_type_diagnostics,
        retained: input
            .inference
            .retained
            .iter()
            .copied()
            .map(category)
            .collect(),
        has_environment: environment.is_some(),
        environment_identity: environment.map_or(0, |environment| environment.identity.get()),
        globals: environment.map_or_else(Vec::new, |environment| environment.globals.clone()),
        definitions: environment.map_or_else(Vec::new, |environment| {
            environment
                .declarations
                .iter()
                .flat_map(|group| &group.definitions)
                .map(|definition| native::NativeDefinition {
                    name: declaration_name(definition.source.version),
                    namespace: definition.namespace.clone(),
                    revision: definition.source.version.revision,
                    text: definition.source.text.to_string(),
                })
                .collect()
        }),
        classes: environment.map_or_else(Vec::new, native_classes),
        has_instances: environment.is_some_and(|environment| environment.instances.is_some()),
        instance_root: environment
            .and_then(|environment| environment.instances.as_ref())
            .map_or_else(String::new, |instances| instances.root.clone()),
        instance_script: environment
            .and_then(|environment| environment.instances.as_ref())
            .map_or_else(String::new, |instances| instances.script.clone()),
        instances: environment
            .and_then(|environment| environment.instances.as_ref())
            .map_or_else(Vec::new, |instances| {
                instances
                    .instances
                    .iter()
                    .map(|instance| native::NativeInstance {
                        identity: instance.identity.clone(),
                        name: instance.name.clone(),
                        classification: instance.class.clone(),
                        parent: instance.parent.clone().unwrap_or_default(),
                        children: instance.children.clone(),
                    })
                    .collect()
            }),
        sites: input.imports.iter().map(import_site).collect(),
    })
}

pub(crate) fn native_classes(input: &EnvironmentInput) -> Vec<native::NativeClass> {
    input
        .capabilities
        .classes
        .iter()
        .map(|class| native::NativeClass {
            name: class.name.clone(),
            service: class.service,
            creatable: class.creatable,
            properties: class
                .properties
                .iter()
                .map(|property| native::NativeProperty {
                    name: property.name.clone(),
                    read: property.read,
                    write: property.write,
                })
                .collect(),
        })
        .collect()
}

pub(crate) fn import_site(input: &ResolvedImport) -> native::NativeSite {
    let (request_kind, request_text) = match &input.site.request {
        ImportRequest::Static(value) => (0, value.clone()),

        ImportRequest::Instance(path) => {
            let mut value = match path.root {
                InstanceRoot::Script => "script",
                InstanceRoot::Game => "game",
                InstanceRoot::Workspace => "workspace",
            }
            .to_owned();

            for step in &path.steps {
                match step {
                    InstanceStep::Parent => value.push_str("/parent"),

                    InstanceStep::Field(name) => {
                        let _ = write!(value, "/field:{}:{name}", name.len());
                    }

                    InstanceStep::Service(name) => {
                        let _ = write!(value, "/service:{}:{name}", name.len());
                    }

                    InstanceStep::Child { name, recursive } => {
                        let _ = write!(
                            value,
                            "/child:{}:{}:{name}",
                            u8::from(*recursive),
                            name.len()
                        );
                    }
                }
            }

            (1, value)
        }

        ImportRequest::Nonconstant => (2, String::new()),
    };

    native::NativeSite {
        call_start: input.site.call.span.start,
        call_end: input.site.call.span.end,
        argument_start: input.site.argument.span.start,
        argument_end: input.site.argument.span.end,
        static_resolution: !matches!(input.outcome, ImportOutcome::Nonconstant),
        target: match input.outcome {
            ImportOutcome::Module(module) => module_name(module),
            _ => String::new(),
        },
        request_kind,
        request_text,
    }
}

pub(crate) const fn purpose(value: AnalysisPurpose) -> u8 {
    match value {
        AnalysisPurpose::Source => 0,
        AnalysisPurpose::Editor => 1,
    }
}

pub(crate) const fn category(value: SemanticCategory) -> u8 {
    match value {
        SemanticCategory::Bindings => 0,
        SemanticCategory::Occurrences => 1,
        SemanticCategory::Types => 2,
        SemanticCategory::Constants => 3,
        SemanticCategory::Dependencies => 4,
        SemanticCategory::Calls => 5,
        SemanticCategory::Effects => 6,
        SemanticCategory::Scopes => 7,
        SemanticCategory::Functions => 8,
        SemanticCategory::Nominals => 9,
        SemanticCategory::Inheritance => 10,
    }
}

pub(crate) fn completion(value: native::NativeCompletion) -> io::Result<Completion> {
    Ok(match value {
        native::NativeCompletion::Complete => Completion::Complete,
        native::NativeCompletion::Cancelled => Completion::Cancelled,
        native::NativeCompletion::Timeout => Completion::TimedOut,

        native::NativeCompletion::Environment => {
            Completion::Incomplete(vec![Reason::UnavailableEnvironment])
        }

        native::NativeCompletion::Analysis => {
            Completion::Incomplete(vec![Reason::UnavailableSemantics])
        }

        native::NativeCompletion::Resource => Completion::Incomplete(vec![Reason::AnalysisLimit]),

        native::NativeCompletion::Unsupported => {
            Completion::Incomplete(vec![Reason::UnsupportedFeature])
        }

        _ => return Err(invalid("unknown native completion tag")),
    })
}

pub(crate) fn diagnostic(
    input: &native::NativeDiagnostic,
    captured: &Captured,
) -> io::Result<Diagnostic> {
    let primary = captured.location(&input.location)?;

    let category = match input.kind {
        native::NativeKind::Syntax => DiagnosticCategory::Syntax,
        native::NativeKind::Type => DiagnosticCategory::Type,
        native::NativeKind::Resolution => DiagnosticCategory::Resolution,
        native::NativeKind::Analysis => DiagnosticCategory::Analysis,
        _ => return Err(invalid("unknown native diagnostic tag")),
    };

    let code = input.code.to_string();

    let mut related = input
        .related
        .iter()
        .map(|related| {
            Ok(RelatedLocation {
                location: captured.location(&related.location)?,
                message: related.message.clone(),
            })
        })
        .collect::<io::Result<Vec<_>>>()?;

    if let Some(request) = &captured.analysis
        && input.message.contains("cyclic")
    {
        for cycle in &request.cycles {
            for edge in &cycle.edges {
                related.push(RelatedLocation {
                    location: edge.site,
                    message: format!(
                        "{} requires {}",
                        module_name(edge.from),
                        module_name(edge.to)
                    ),
                });
            }
        }
    }

    Ok(Diagnostic {
        identity: FindingIdentity::new("luau", captured.inputs(), &code, primary),
        code,
        category,
        severity: Severity::Error,
        message: input.message.clone(),
        primary,
        related,
        tags: Vec::new(),
    })
}

pub(crate) fn analysis_result(
    input: native::NativeAnalysisResult,
    captured: &Captured,
) -> io::Result<WireAnalysisResult> {
    let request = captured
        .analysis
        .as_ref()
        .ok_or_else(|| invalid("analysis input is unavailable"))?;

    if input.frontend_revision == 0 || input.modules.len() != request.modules.len() {
        return Err(invalid(
            "native analysis result omits its established revision or modules",
        ));
    }

    let mut modules = Vec::with_capacity(input.modules.len());

    for (output, module) in input.modules.into_iter().zip(&request.modules) {
        modules.push(native_module_analysis(output, module, captured)?);
    }

    Ok(WireAnalysisResult {
        closure: request.closure,
        frontend_revision: input.frontend_revision,
        modules,
        completion: completion(input.completion)?,
    })
}

fn native_module_analysis(
    output: native::NativeModuleAnalysis,
    module: &ModuleInput,
    captured: &Captured,
) -> io::Result<WireModuleAnalysis> {
    if output.name != module_name(module.analysis.module)
        || output.logical_revision != module.analysis.revision
        || output.inferred && !module.inference.required
        || !output.environment_diagnostics.is_empty() && module.environment.is_none()
    {
        return Err(invalid(
            "native analysis result disagrees with captured identities or requirements",
        ));
    }

    if !output.parsed
        && (output.inferred
            || output.recovered_syntax
            || !output.directives.is_empty()
            || output.completion == native::NativeCompletion::Complete)
    {
        return Err(invalid(
            "native module claims established state without parsing",
        ));
    }

    let diagnostics = output
        .diagnostics
        .iter()
        .map(|output| diagnostic(output, captured))
        .collect::<io::Result<Vec<_>>>()?;

    let environment_diagnostics = output
        .environment_diagnostics
        .iter()
        .map(|output| diagnostic(output, captured))
        .collect::<io::Result<Vec<_>>>()?;

    let completed = completion(output.completion)?;

    let parsed = ParsedMetadata {
        input: module.source.clone(),
        diagnostics: diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == DiagnosticCategory::Syntax)
            .cloned()
            .collect(),
        directives: output
            .directives
            .into_iter()
            .map(|directive| {
                Ok(SourceDirective {
                    location: captured.location(&directive.location)?,
                    text: directive.text,
                })
            })
            .collect::<io::Result<_>>()?,
        imports: module
            .imports
            .iter()
            .map(|import| import.site.clone())
            .collect(),
        recovered_syntax: output.recovered_syntax,
        completion: if output.parsed {
            Completion::Complete
        } else {
            completed.clone()
        },
    };

    let environment = module
        .environment
        .as_ref()
        .map(|environment| EnvironmentMetadata {
            input: environment.clone(),
            diagnostics: environment_diagnostics,
            availability: if output.environment_available {
                Availability::Available
            } else {
                Availability::Unavailable(
                    completion_reason(&completed).unwrap_or(Reason::UnavailableEnvironment),
                )
            },
            completion: if output.environment_available {
                Completion::Complete
            } else if matches!(completed, Completion::Complete) {
                Completion::Incomplete(vec![Reason::UnavailableEnvironment])
            } else {
                completed.clone()
            },
        });

    Ok(WireModuleAnalysis {
        analysis: module.analysis,
        parsed,
        environment,
        diagnostics,
        availability: semantic_availability(
            module,
            output.inferred,
            output.semantic_complete,
            output.recovered_syntax,
            &completed,
        ),
        completion: completed,
    })
}

pub(crate) fn semantic_availability(
    module: &ModuleInput,
    inferred: bool,
    semantic_complete: bool,
    recovered: bool,
    completed: &Completion,
) -> SemanticAvailability {
    let reason = completion_reason(completed).unwrap_or(Reason::UnavailableSemantics);

    SemanticAvailability {
        categories: module
            .inference
            .retained
            .iter()
            .map(|category| {
                let status = if !inferred {
                    Availability::Unavailable(reason)
                } else if recovered
                    || !semantic_complete
                    || !matches!(completed, Completion::Complete)
                {
                    Availability::Partial(reason)
                } else if matches!(
                    category,
                    SemanticCategory::Constants
                        | SemanticCategory::Dependencies
                        | SemanticCategory::Calls
                        | SemanticCategory::Effects
                        | SemanticCategory::Inheritance
                ) {
                    Availability::Partial(Reason::UnavailableSemantics)
                } else {
                    Availability::Available
                };

                (*category, status)
            })
            .collect(),
    }
}

fn completion_reason(completed: &Completion) -> Option<Reason> {
    match completed {
        Completion::Complete => None,

        Completion::Incomplete(reasons) => reasons
            .first()
            .copied()
            .or(Some(Reason::FailedPrerequisite)),

        Completion::Cancelled | Completion::TimedOut => Some(Reason::FailedPrerequisite),
    }
}

pub(crate) fn result_availability(
    established: &SemanticAvailability,
    completed: &Completion,
) -> SemanticAvailability {
    let Some(reason) = completion_reason(completed) else {
        return established.clone();
    };

    if matches!(completed, Completion::Cancelled | Completion::TimedOut) {
        return SemanticAvailability {
            categories: established
                .categories
                .keys()
                .map(|category| (*category, Availability::Unavailable(reason)))
                .collect(),
        };
    }

    SemanticAvailability {
        categories: established
            .categories
            .iter()
            .map(|(category, status)| {
                (
                    *category,
                    match status {
                        Availability::Unavailable(_) => Availability::Unavailable(reason),

                        Availability::Available | Availability::Partial(_) => {
                            Availability::Partial(reason)
                        }
                    },
                )
            })
            .collect(),
    }
}

pub(crate) fn validate_reference<'captured>(
    input: &WireReference,
    captured: &'captured Captured,
    revision: u64,
) -> io::Result<&'captured ModuleInput> {
    let request = captured
        .analysis
        .as_ref()
        .ok_or_else(|| invalid("native analysis is unavailable"))?;

    if input.frontend_revision != revision || input.purpose != request.purpose {
        return Err(Reason::StaleQuery.error());
    }

    captured.module(input.analysis)
}

pub(crate) fn facts_input(input: &WireFactsInput) -> io::Result<native::NativeFactsInput> {
    let mut selected = BTreeSet::new();

    for category in &input.categories {
        if !selected.insert(*category) {
            return Err(invalid("duplicate semantic category request"));
        }
    }

    Ok(native::NativeFactsInput {
        name: module_name(input.analysis.analysis.module),
        logical_revision: input.analysis.analysis.revision,
        frontend_revision: input.analysis.frontend_revision,
        purpose: purpose(input.analysis.purpose),
        categories: input.categories.iter().copied().map(category).collect(),
    })
}

pub(crate) fn lint_input(input: &WireLintInput) -> io::Result<native::NativeLintInput> {
    let mut selected = BTreeSet::new();

    for warning in &input.warnings.enabled {
        if warning.name.is_empty() || !selected.insert(&warning.name) {
            return Err(invalid("empty or duplicate upstream warning request"));
        }
    }

    Ok(native::NativeLintInput {
        name: module_name(input.analysis.analysis.module),
        logical_revision: input.analysis.analysis.revision,
        frontend_revision: input.analysis.frontend_revision,
        purpose: purpose(input.analysis.purpose),
        mode: match input.warnings.language_mode {
            LanguageMode::NoCheck => 0,
            LanguageMode::Nonstrict => 1,
            LanguageMode::Strict => 2,
            LanguageMode::Definition => 3,
        },
        warnings: input
            .warnings
            .enabled
            .iter()
            .map(|warning| warning.name.clone())
            .collect(),
    })
}

pub(crate) fn position_input(
    input: &WirePositionInput,
    captured: &Captured,
) -> io::Result<native::NativePositionInput> {
    let name = module_name(input.analysis.analysis.module);
    let position = captured.position(&name, input.offset)?;

    Ok(native::NativePositionInput {
        name,
        logical_revision: input.analysis.analysis.revision,
        frontend_revision: input.analysis.frontend_revision,
        purpose: purpose(input.analysis.purpose),
        offset: input.offset,
        line: u32::try_from(position.line).map_err(invalid)?,
        column: u32::try_from(position.column).map_err(invalid)?,
    })
}

fn source_identity(location: Location, kind: SemanticKind) -> SemanticIdentity {
    SemanticIdentity::Source {
        location,
        kind,
        ordinal: 0,
    }
}

fn reference_kind(reference: &native::NativeReference) -> io::Result<SemanticKind> {
    Ok(match reference.kind {
        0 if reference.parameter => SemanticKind::Parameter,
        0 if reference.function => SemanticKind::Function,
        0 => SemanticKind::Binding,
        1 => SemanticKind::Type,
        2 | 4 => SemanticKind::Property,
        3 => SemanticKind::Receiver,
        _ => return Err(invalid("unknown native reference category")),
    })
}

fn binding_kind(value: u8) -> io::Result<BindingKind> {
    Ok(match value {
        0 => BindingKind::Local,
        1 => BindingKind::Parameter,
        2 => BindingKind::Loop,
        3 => BindingKind::Function,
        4 => BindingKind::Global,
        5 => BindingKind::Property,
        6 => BindingKind::Type,
        7 => BindingKind::Receiver,
        _ => return Err(invalid("unknown native binding kind")),
    })
}

fn environment_identity(
    module: &ModuleInput,
    documentation: &str,
    name: &str,
    declaration: Option<Location>,
    kind: SemanticKind,
) -> io::Result<SemanticIdentity> {
    let environment = module
        .environment
        .as_ref()
        .ok_or_else(|| invalid("native environment binding has no captured environment"))?;

    if let Some(declaration) = declaration {
        let definition = environment
            .declarations
            .iter()
            .flat_map(|group| &group.definitions)
            .find(|definition| definition.source.version == declaration.source)
            .ok_or_else(|| invalid("native environment declaration is not captured"))?;

        return Ok(SemanticIdentity::Environment {
            environment: environment.identity,
            namespace: definition.namespace.clone(),
            name: name.to_owned(),
            kind,
        });
    }

    let namespace = if kind == SemanticKind::Binding
        && environment.globals.iter().any(|global| global == name)
    {
        ""
    } else {
        "@luau"
    };

    let (namespace, qualified_name) = documentation
        .split_once("/global/")
        .or_else(|| documentation.split_once("/type/"))
        .or_else(|| documentation.split_once("/globaltype/"))
        .map_or((namespace, name), |(namespace, qualified_name)| {
            (namespace, qualified_name)
        });

    Ok(SemanticIdentity::Environment {
        environment: environment.identity,
        namespace: namespace.to_owned(),
        name: qualified_name.to_owned(),
        kind,
    })
}

fn type_fact(
    display: String,
    is_any: bool,
    declarations: &[native::NativeLocation],
    captured: &Captured,
) -> io::Result<TypeFact> {
    if display.is_empty() {
        return Err(invalid("established native type has empty display text"));
    }

    let mut declarations = declarations
        .iter()
        .map(|location| {
            Ok(source_identity(
                captured.location(location)?,
                SemanticKind::Type,
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;

    declarations.sort();
    declarations.dedup();

    Ok(TypeFact {
        display,
        is_any,
        declarations,
    })
}

fn signatures(
    input: &native::NativeSignatures,
    captured: &Captured,
) -> io::Result<Knowledge<Vec<CallableSignature>>> {
    if !input.found {
        if !input.entries.is_empty() || input.has_active {
            return Err(invalid(
                "absent native signature carries entries or selection",
            ));
        }

        return Ok(Knowledge::Unavailable(Reason::UnavailableSemantics));
    }

    captured.location(&input.location)?;

    if input.entries.is_empty() || input.has_active && input.active >= input.entries.len() {
        return Err(invalid("native signature selection is invalid"));
    }

    Ok(Knowledge::Known(
        input
            .entries
            .iter()
            .map(|signature| {
                if signature.label.is_empty() {
                    return Err(invalid("established native signature has an empty label"));
                }

                let mut end = 0;
                let mut parameters = Vec::with_capacity(signature.parameters.len());

                for parameter in &signature.parameters {
                    if parameter.start < end
                        || parameter.start >= parameter.end
                        || !signature.label.is_char_boundary(parameter.start)
                        || !signature.label.is_char_boundary(parameter.end)
                    {
                        return Err(invalid("native signature parameter range is invalid"));
                    }

                    end = parameter.end;

                    parameters.push(SignatureParameter {
                        label_range: [parameter.start, parameter.end],
                        declaration: parameter
                            .has_declaration
                            .then(|| captured.location(&parameter.declaration))
                            .transpose()?,
                    });
                }

                if signature.has_minimum_arguments && signature.minimum_arguments > parameters.len()
                {
                    return Err(invalid(
                        "native signature minimum exceeds its parameter count",
                    ));
                }

                Ok(CallableSignature {
                    label: signature.label.clone(),
                    parameters,
                    variadic: signature.variadic,
                    minimum_arguments: if signature.has_minimum_arguments {
                        Knowledge::Known(signature.minimum_arguments)
                    } else {
                        Knowledge::Unavailable(Reason::UnavailableSemantics)
                    },
                    documentation: (!input.documentation.is_empty())
                        .then(|| input.documentation.clone()),
                })
            })
            .collect::<io::Result<_>>()?,
    ))
}

fn hierarchy_identities(
    input: &native::NativeHierarchy,
    captured: &Captured,
) -> io::Result<Vec<SemanticIdentity>> {
    let mut identities = Vec::with_capacity(input.items.len());

    for item in &input.items {
        let location = captured.location(&item.location)?;

        let selection = Location {
            source: location.source,
            span: Span {
                start: item.selection_start,
                end: item.selection_end,
            },
        };

        captured.validate_location(selection)?;

        let body = Location {
            source: location.source,
            span: Span {
                start: item.body_start,
                end: item.body_end,
            },
        };

        captured.validate_location(body)?;

        if selection.span.start < location.span.start
            || selection.span.end > location.span.end
            || body.span.start < location.span.start
            || body.span.end > location.span.end
        {
            return Err(invalid(
                "native hierarchy selection or body escapes its declaration",
            ));
        }

        let kind = match item.kind {
            0 => SemanticKind::Module,
            1 => SemanticKind::Function,
            2 => SemanticKind::Class,
            _ => return Err(invalid("unknown native hierarchy kind")),
        };

        identities.push(source_identity(
            if item.kind == 1 && selection.span.start != selection.span.end {
                selection
            } else {
                location
            },
            kind,
        ));
    }

    for call in &input.calls {
        if call.caller >= identities.len() || call.callee >= identities.len() {
            return Err(invalid("native call target index is invalid"));
        }

        captured.location(&call.location)?;
    }

    for inheritance in &input.inheritance {
        if inheritance.child >= identities.len() || inheritance.parent >= identities.len() {
            return Err(invalid("native inheritance index is invalid"));
        }
    }

    if input
        .selected
        .iter()
        .any(|selected| *selected >= identities.len())
    {
        return Err(invalid("native selected hierarchy index is invalid"));
    }

    Ok(identities)
}

pub(crate) fn facts_result(
    input: &native::NativeFactsResult,
    request: &WireFactsInput,
    captured: &Captured,
    established: &SemanticAvailability,
) -> io::Result<FactsResult> {
    let module = captured.module(request.analysis.analysis)?;
    let identities = hierarchy_identities(&input.hierarchy, captured)?;
    let relationships = fact_identities(input, module, captured, &identities)?;
    let mut facts = SemanticFacts::default();
    reference_facts(input, module, captured, &relationships, &mut facts)?;
    environment_facts(input, module, captured, &relationships, &mut facts)?;
    scope_facts(input, captured, &mut facts)?;
    expression_facts(input, module, captured, &relationships, &mut facts)?;
    call_facts(input, module, captured, &identities, &mut facts)?;
    hierarchy_facts(input, captured, &identities, &mut facts)?;
    sort_facts(&mut facts);
    let availability = fact_coverage(input, request, established, &facts);
    let native_completed = completion(input.completion)?;
    let availability = result_availability(&availability, &native_completed);
    select_facts(request, &mut facts);
    let mut completed = native_completed;

    for status in availability.categories.values() {
        if let Availability::Partial(reason) | Availability::Unavailable(reason) = status {
            completed = completed.combine(Completion::Incomplete(vec![*reason]));
        }
    }

    Ok(FactsResult {
        analysis: request.analysis.analysis,
        facts,
        availability,
        diagnostics: Vec::new(),
        completion: completed,
    })
}

fn fact_identities(
    input: &native::NativeFactsResult,
    module: &ModuleInput,
    captured: &Captured,
    identities: &[SemanticIdentity],
) -> io::Result<FactIdentities> {
    let mut declarations = BTreeMap::new();
    let mut environment_bindings = BTreeMap::new();

    for binding in &input.environment_bindings {
        let kind = if binding.kind == 6 {
            SemanticKind::Type
        } else if binding.kind == 4 {
            SemanticKind::Binding
        } else {
            return Err(invalid("invalid native environment binding category"));
        };

        let declaration = binding
            .has_declaration
            .then(|| captured.location(&binding.declaration))
            .transpose()?;

        let identity = environment_identity(
            module,
            &binding.documentation,
            &binding.name,
            declaration,
            kind,
        )?;

        if let Some(declaration) = declaration {
            declarations.insert(declaration, identity.clone());
        }

        environment_bindings.insert((binding.name.clone(), kind), identity);
    }

    for reference in &input.references {
        if reference.declaration {
            declarations.insert(
                captured.location(&reference.location)?,
                source_identity(
                    captured.location(&reference.location)?,
                    reference_kind(reference)?,
                ),
            );
        }
    }

    for (item, identity) in input.hierarchy.items.iter().zip(identities) {
        let location = captured.location(&item.location)?;

        declarations
            .entry(location)
            .or_insert_with(|| identity.clone());

        if item.selection_start != item.selection_end {
            declarations
                .entry(Location {
                    source: location.source,
                    span: Span {
                        start: item.selection_start,
                        end: item.selection_end,
                    },
                })
                .or_insert_with(|| identity.clone());
        }
    }

    Ok(FactIdentities {
        declarations,
        environment_bindings,
    })
}

fn reference_facts(
    input: &native::NativeFactsResult,
    module: &ModuleInput,
    captured: &Captured,
    relationships: &FactIdentities,
    facts: &mut SemanticFacts,
) -> io::Result<()> {
    let declarations = &relationships.declarations;
    let environment_bindings = &relationships.environment_bindings;

    for reference in &input.references {
        let location = captured.location(&reference.location)?;
        let kind = reference_kind(reference)?;

        let environment_kind = if reference.kind == 0 {
            SemanticKind::Binding
        } else {
            kind
        };

        let targets = if !reference.targets.is_empty() {
            Knowledge::Known(
                reference
                    .targets
                    .iter()
                    .map(|target| {
                        let location = captured.location(target)?;

                        Ok(declarations
                            .get(&location)
                            .cloned()
                            .unwrap_or_else(|| source_identity(location, kind)))
                    })
                    .collect::<io::Result<Vec<_>>>()?,
            )
        } else if let Some(identity) =
            environment_bindings.get(&(reference.name.clone(), environment_kind))
        {
            Knowledge::Known(vec![identity.clone()])
        } else if !reference.documentation.is_empty() {
            Knowledge::Known(vec![environment_identity(
                module,
                &reference.documentation,
                &reference.name,
                None,
                kind,
            )?])
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        };

        let role = if reference.kind == 4 {
            OccurrenceRole::Method
        } else {
            match kind {
                SemanticKind::Parameter => OccurrenceRole::Parameter,
                SemanticKind::Function => OccurrenceRole::Function,
                SemanticKind::Property | SemanticKind::Receiver => OccurrenceRole::Property,
                SemanticKind::Type | SemanticKind::Class => OccurrenceRole::Type,
                _ => OccurrenceRole::Variable,
            }
        };

        facts.occurrences.push(OccurrenceFact {
            location,
            targets,
            declaration: reference.declaration,
            access: match (reference.read, reference.write) {
                (false, false) => OccurrenceAccess::None,
                (true, false) => OccurrenceAccess::Read,
                (false, true) => OccurrenceAccess::Write,
                (true, true) => OccurrenceAccess::ReadWrite,
            },
            role,
            deprecated: reference.deprecated,
        });

        if reference.declaration {
            facts
                .bindings
                .push(source_binding(reference, captured, location, kind)?);
        }
    }

    Ok(())
}

fn source_binding(
    reference: &native::NativeReference,
    captured: &Captured,
    location: Location,
    kind: SemanticKind,
) -> io::Result<BindingFact> {
    let scope = source_identity(captured.location(&reference.scope)?, SemanticKind::Scope);

    Ok(BindingFact {
        identity: source_identity(location, kind),
        declaration: Knowledge::Known(Some(location)),
        name: reference.name.clone(),
        kind: binding_kind(reference.binding_kind)?,
        annotation: reference.annotation,
        scope,
        shadows: Knowledge::Known(
            reference
                .has_shadow
                .then(|| {
                    captured
                        .location(&reference.shadow)
                        .map(|shadow| source_identity(shadow, SemanticKind::Binding))
                })
                .transpose()?,
        ),
        type_information: if reference.has_type {
            Knowledge::Known(type_fact(
                reference.type_text.clone(),
                reference.is_any,
                &reference.type_declarations,
                captured,
            )?)
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        },
        documentation: (!reference.documentation.is_empty())
            .then(|| reference.documentation.clone()),
        deprecated: reference.deprecated,
        language_identity: None,
    })
}

fn environment_facts(
    input: &native::NativeFactsResult,
    module: &ModuleInput,
    captured: &Captured,
    relationships: &FactIdentities,
    facts: &mut SemanticFacts,
) -> io::Result<()> {
    let environment_bindings = &relationships.environment_bindings;

    let root = source_identity(
        Location::new(
            &module.source.source,
            Span {
                start: 0,
                end: module.source.source.text.len(),
            },
        )?,
        SemanticKind::Scope,
    );

    for binding in &input.environment_bindings {
        let kind = if binding.kind == 6 {
            SemanticKind::Type
        } else {
            SemanticKind::Binding
        };

        let identity = environment_bindings
            .get(&(binding.name.clone(), kind))
            .cloned()
            .ok_or_else(|| invalid("native environment identity is missing"))?;

        let declaration = binding
            .has_declaration
            .then(|| captured.location(&binding.declaration))
            .transpose()?;

        let language_identity = if binding.documentation.starts_with("@luau/") {
            Some(LanguageIdentity::Builtin(binding.name.clone()))
        } else if module
            .environment
            .as_ref()
            .is_some_and(|environment| !environment.capabilities.classes.is_empty())
            && binding.documentation.starts_with("@roblox/")
        {
            Some(LanguageIdentity::Roblox(binding.name.clone()))
        } else {
            None
        };

        facts.bindings.push(BindingFact {
            identity,
            declaration: Knowledge::Known(declaration),
            name: binding.name.clone(),
            kind: binding_kind(binding.kind)?,
            annotation: false,
            scope: root.clone(),
            shadows: Knowledge::Known(None),
            type_information: Knowledge::Known(type_fact(
                binding.type_text.clone(),
                binding.is_any,
                &[],
                captured,
            )?),
            documentation: (!binding.documentation.is_empty())
                .then(|| binding.documentation.clone()),
            deprecated: binding.deprecated,
            language_identity,
        });
    }

    Ok(())
}

fn scope_facts(
    input: &native::NativeFactsResult,
    captured: &Captured,
    facts: &mut SemanticFacts,
) -> io::Result<()> {
    for scope in &input.scopes {
        let location = captured.location(&scope.location)?;

        facts.scopes.push(ScopeFact {
            identity: source_identity(location, SemanticKind::Scope),
            location,
            parent: scope
                .has_parent
                .then(|| {
                    captured
                        .location(&scope.parent)
                        .map(|parent| source_identity(parent, SemanticKind::Scope))
                })
                .transpose()?,
        });
    }

    Ok(())
}

fn expression_facts(
    input: &native::NativeFactsResult,
    module: &ModuleInput,
    captured: &Captured,
    relationships: &FactIdentities,
    facts: &mut SemanticFacts,
) -> io::Result<()> {
    let declarations = &relationships.declarations;

    for expression in &input.expressions {
        let location = captured.location(&expression.location)?;

        let constant = match expression.constant_kind {
            0 => Knowledge::Unavailable(Reason::UnavailableSemantics),
            1 => Knowledge::Known(Some(ConstantValue::Nil)),
            2 => Knowledge::Known(Some(ConstantValue::Boolean(expression.constant_boolean))),

            3 if expression.constant_number.is_finite() => {
                Knowledge::Known(Some(ConstantValue::Number(expression.constant_number)))
            }

            4 => Knowledge::Known(Some(ConstantValue::String(
                expression.constant_text.clone(),
            ))),

            5 => Knowledge::Known(None),
            _ => return Err(invalid("invalid native constant tag or number")),
        };

        let local_dependency = if expression.has_dependency {
            let dependency = captured.location(&expression.dependency)?;

            Some(
                declarations
                    .get(&dependency)
                    .cloned()
                    .unwrap_or_else(|| source_identity(dependency, SemanticKind::Binding)),
            )
        } else {
            None
        };

        let dependencies = if !expression.builtin_dependency.is_empty() {
            let kind = if expression.builtin_dependency == "math" {
                SemanticKind::Binding
            } else if expression.builtin_dependency == "math.abs" {
                SemanticKind::Property
            } else {
                return Err(invalid("unsupported native builtin effect identity"));
            };

            Knowledge::Known(vec![environment_identity(
                module,
                &format!("@luau/global/{}", expression.builtin_dependency),
                &expression.builtin_dependency,
                None,
                kind,
            )?])
        } else if expression.has_dependency {
            Knowledge::Known(
                local_dependency
                    .clone()
                    .filter(|_| expression.read)
                    .into_iter()
                    .collect(),
            )
        } else if expression.literal {
            Knowledge::Known(Vec::new())
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        };

        let effects = if expression.known_effects {
            let reads = match &dependencies {
                Knowledge::Known(dependencies) => dependencies.clone(),

                Knowledge::Unavailable(_) => {
                    return Err(invalid("native effect proof lacks dependency evidence"));
                }
            };

            Knowledge::Known(EffectFact {
                reads,
                writes: local_dependency
                    .filter(|_| expression.write)
                    .into_iter()
                    .collect(),
                may_throw: false,
                may_yield: false,
                may_allocate: false,
                determinism: Determinism::Deterministic,
                control_flow: ControlFlow::Preserved,
            })
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        };

        facts.expressions.push(ExpressionFact {
            location,
            type_information: if expression.has_type {
                Knowledge::Known(type_fact(
                    expression.type_text.clone(),
                    expression.is_any,
                    &expression.type_declarations,
                    captured,
                )?)
            } else {
                Knowledge::Unavailable(Reason::UnavailableSemantics)
            },
            constant,
            dependencies,
            effects,
        });
    }

    Ok(())
}

fn call_facts(
    input: &native::NativeFactsResult,
    module: &ModuleInput,
    captured: &Captured,
    identities: &[SemanticIdentity],
    facts: &mut SemanticFacts,
) -> io::Result<()> {
    for evidence in &input.calls {
        let location = captured.location(&evidence.location)?;
        let callee = captured.location(&evidence.callee)?;

        let mut targets = input
            .hierarchy
            .calls
            .iter()
            .filter(|call| {
                captured
                    .location(&call.location)
                    .is_ok_and(|range| range == callee)
            })
            .map(|call| identities[call.callee].clone())
            .collect::<Vec<_>>();

        if targets.is_empty() {
            for occurrence in &facts.occurrences {
                if occurrence.location.source == callee.source
                    && occurrence.location.span.start >= callee.span.start
                    && occurrence.location.span.end == callee.span.end
                    && let Knowledge::Known(resolved) = &occurrence.targets
                {
                    targets.extend(resolved.iter().cloned());
                }
            }
        }

        targets.sort();
        targets.dedup();
        let enclosing_function = enclosing_caller(input, module, identities, location);
        let signature_information = signatures(&evidence.signatures, captured)?;

        let selected = if let Knowledge::Known(signatures) = &signature_information {
            if evidence.signatures.has_active {
                signatures.get(evidence.signatures.active)
            } else if signatures.len() == 1 {
                signatures.first()
            } else {
                None
            }
        } else {
            None
        };

        let arguments = evidence
            .arguments
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                Ok(ArgumentRelation {
                    argument: captured.location(argument)?,
                    parameter: selected.map_or(
                        Knowledge::Unavailable(Reason::UnavailableSemantics),
                        |signature| {
                            Knowledge::Known((index < signature.parameters.len()).then_some(index))
                        },
                    ),
                })
            })
            .collect::<io::Result<_>>()?;

        let effects = facts
            .expressions
            .iter()
            .find(|expression| expression.location == location)
            .map_or(
                Knowledge::Unavailable(Reason::UnavailableSemantics),
                |expression| expression.effects.clone(),
            );

        facts.calls.push(CallFact {
            location,
            callee,
            caller: enclosing_function,
            targets: if targets.is_empty() {
                Knowledge::Unavailable(Reason::UnavailableSemantics)
            } else {
                Knowledge::Known(targets)
            },
            signatures: signature_information,
            arguments,
            effects,
        });
    }

    Ok(())
}

fn enclosing_caller(
    input: &native::NativeFactsResult,
    module: &ModuleInput,
    identities: &[SemanticIdentity],
    location: Location,
) -> SemanticIdentity {
    input
        .hierarchy
        .items
        .iter()
        .zip(identities)
        .filter(|(item, _)| {
            item.kind == 1
                && item.location.module == module_name(module.analysis.module)
                && item.body_start <= location.span.start
                && location.span.end <= item.body_end
        })
        .min_by_key(|(item, _)| item.body_end - item.body_start)
        .map_or_else(
            || {
                source_identity(
                    Location {
                        source: module.source.source.version,
                        span: Span {
                            start: 0,
                            end: module.source.source.text.len(),
                        },
                    },
                    SemanticKind::Module,
                )
            },
            |(_, identity)| identity.clone(),
        )
}

fn hierarchy_facts(
    input: &native::NativeFactsResult,
    captured: &Captured,
    identities: &[SemanticIdentity],
    facts: &mut SemanticFacts,
) -> io::Result<()> {
    for (item, identity) in input.hierarchy.items.iter().zip(identities) {
        let location = captured.location(&item.location)?;

        let selection = Location {
            source: location.source,
            span: Span {
                start: item.selection_start,
                end: item.selection_end,
            },
        };

        if item.kind == 1 {
            let body = Location {
                source: location.source,
                span: Span {
                    start: item.body_start,
                    end: item.body_end,
                },
            };

            let scope = facts
                .scopes
                .iter()
                .filter(|scope| {
                    scope.location.source == body.source
                        && scope.location.span.start <= body.span.start
                        && body.span.end <= scope.location.span.end
                })
                .min_by_key(|scope| scope.location.span.end - scope.location.span.start)
                .map_or_else(
                    || source_identity(body, SemanticKind::Scope),
                    |scope| scope.identity.clone(),
                );

            facts.functions.push(FunctionFact {
                identity: identity.clone(),
                location,
                selection,
                body,
                scope,
                signatures: signatures(&item.signatures, captured)?,
            });
        } else if item.kind == 2 {
            facts.nominals.push(NominalFact {
                identity: identity.clone(),
                name: item.name.clone(),
                declaration: location,
                selection,
                documentation: None,
                deprecated: false,
            });
        }
    }

    for edge in &input.hierarchy.inheritance {
        facts.inheritance.push(InheritanceFact {
            child: identities[edge.child].clone(),
            parent: identities[edge.parent].clone(),
        });
    }

    Ok(())
}

fn sort_facts(facts: &mut SemanticFacts) {
    facts
        .bindings
        .sort_by(|left, right| left.identity.cmp(&right.identity));

    facts
        .bindings
        .dedup_by(|left, right| left.identity == right.identity);

    facts
        .occurrences
        .sort_by_key(|occurrence| occurrence.location);

    facts.occurrences.dedup();

    facts
        .expressions
        .sort_by_key(|expression| expression.location);

    facts.calls.sort_by_key(|call| call.location);
    facts.scopes.sort_by_key(|scope| scope.location);
    facts.functions.sort_by_key(|function| function.location);
    facts.nominals.sort_by_key(|nominal| nominal.declaration);
}

fn fact_coverage(
    input: &native::NativeFactsResult,
    request: &WireFactsInput,
    established: &SemanticAvailability,
    facts: &SemanticFacts,
) -> SemanticAvailability {
    SemanticAvailability {
        categories: request
            .categories
            .iter()
            .map(|category| {
                let status = established
                    .categories
                    .get(category)
                    .copied()
                    .unwrap_or(Availability::Unavailable(Reason::UnavailableSemantics));

                let known = match category {
                    SemanticCategory::Bindings => facts.bindings.iter().all(|binding| {
                        matches!(binding.declaration, Knowledge::Known(_))
                            && matches!(binding.shadows, Knowledge::Known(_))
                    }),

                    SemanticCategory::Occurrences => facts
                        .occurrences
                        .iter()
                        .all(|occurrence| matches!(occurrence.targets, Knowledge::Known(_))),

                    SemanticCategory::Types => {
                        facts.expressions.iter().all(|expression| {
                            matches!(expression.type_information, Knowledge::Known(_))
                        }) && facts
                            .bindings
                            .iter()
                            .all(|binding| matches!(binding.type_information, Knowledge::Known(_)))
                    }

                    SemanticCategory::Constants => facts
                        .expressions
                        .iter()
                        .all(|expression| matches!(expression.constant, Knowledge::Known(_))),

                    SemanticCategory::Dependencies => facts
                        .expressions
                        .iter()
                        .all(|expression| matches!(expression.dependencies, Knowledge::Known(_))),

                    SemanticCategory::Effects => facts
                        .expressions
                        .iter()
                        .all(|expression| matches!(expression.effects, Knowledge::Known(_))),

                    SemanticCategory::Calls => facts.calls.iter().all(|call| {
                        matches!(call.targets, Knowledge::Known(_))
                            && matches!(call.signatures, Knowledge::Known(_))
                            && call
                                .arguments
                                .iter()
                                .all(|argument| matches!(argument.parameter, Knowledge::Known(_)))
                    }),

                    SemanticCategory::Functions => facts
                        .functions
                        .iter()
                        .all(|function| matches!(function.signatures, Knowledge::Known(_))),

                    SemanticCategory::Scopes
                    | SemanticCategory::Nominals
                    | SemanticCategory::Inheritance => true,
                };

                let proven = input.evidence_complete
                    && input.completion == native::NativeCompletion::Complete
                    && known
                    && match category {
                        SemanticCategory::Calls
                        | SemanticCategory::Dependencies
                        | SemanticCategory::Effects => true,

                        SemanticCategory::Inheritance => input.hierarchy.inheritance_complete,
                        _ => false,
                    };

                (
                    *category,
                    if proven && matches!(status, Availability::Partial(_)) {
                        Availability::Available
                    } else if !known && status == Availability::Available {
                        Availability::Partial(Reason::UnavailableSemantics)
                    } else {
                        status
                    },
                )
            })
            .collect(),
    }
}

fn select_facts(request: &WireFactsInput, facts: &mut SemanticFacts) {
    let selected = request.categories.iter().copied().collect::<BTreeSet<_>>();

    if !selected.contains(&SemanticCategory::Bindings) {
        facts.bindings.clear();
    }

    if !selected.contains(&SemanticCategory::Occurrences) {
        facts.occurrences.clear();
    }

    if !selected.iter().any(|category| {
        matches!(
            category,
            SemanticCategory::Types
                | SemanticCategory::Constants
                | SemanticCategory::Dependencies
                | SemanticCategory::Effects
        )
    }) {
        facts.expressions.clear();
    }

    if !selected.contains(&SemanticCategory::Calls) {
        facts.calls.clear();
    }

    if !selected.contains(&SemanticCategory::Scopes) {
        facts.scopes.clear();
    }

    if !selected.contains(&SemanticCategory::Functions) {
        facts.functions.clear();
    }

    if !selected.contains(&SemanticCategory::Nominals) {
        facts.nominals.clear();
    }

    if !selected.contains(&SemanticCategory::Inheritance) {
        facts.inheritance.clear();
    }
}

pub(crate) fn inspection_result(
    input: native::NativeInspectionResult,
    request: &WirePositionInput,
    captured: &Captured,
    established: &SemanticAvailability,
) -> io::Result<InspectionResult> {
    let identities = hierarchy_identities(&input.hierarchy, captured)?;

    let definitions = input
        .definitions
        .iter()
        .map(|location| captured.location(location))
        .collect::<io::Result<Vec<_>>>()?;

    let type_definitions = input
        .type_definitions
        .iter()
        .map(|location| captured.location(location))
        .collect::<io::Result<Vec<_>>>()?;

    let signature_information = signatures(&input.signatures, captured)?;
    let completed = completion(input.completion)?;
    let mut availability = result_availability(established, &completed);

    if input.signatures.found
        && matches!(
            input.completion,
            native::NativeCompletion::Complete | native::NativeCompletion::Analysis
        )
        && established
            .categories
            .get(&SemanticCategory::Calls)
            .is_some_and(|status| !matches!(status, Availability::Unavailable(_)))
    {
        availability
            .categories
            .insert(SemanticCategory::Calls, Availability::Available);
    }

    let occurrence = if input.hover.found
        || input.signatures.found
        || !definitions.is_empty()
        || !input.hierarchy.selected.is_empty()
    {
        Some(inspection_position(
            input,
            request,
            captured,
            PositionEvidence {
                identities,
                definitions,
                type_definitions,
                signature_information,
            },
            established,
            &completed,
        )?)
    } else {
        None
    };

    Ok(InspectionResult {
        analysis: request.analysis.analysis,
        occurrence,
        availability,
        diagnostics: Vec::new(),
        completion: completed,
    })
}

fn inspection_position(
    input: native::NativeInspectionResult,
    request: &WirePositionInput,
    captured: &Captured,
    evidence: PositionEvidence,
    established: &SemanticAvailability,
    completed: &Completion,
) -> io::Result<PositionFacts> {
    let module = captured.module(request.analysis.analysis)?;

    let PositionEvidence {
        identities,
        definitions,
        type_definitions,
        signature_information,
    } = evidence;

    let available = |category| {
        *completed == Completion::Complete
            && established.categories.get(&category) == Some(&Availability::Available)
    };

    let location = if input.hover.found {
        captured.location(&input.hover.location)?
    } else if input.signatures.found {
        captured.location(&input.signatures.location)?
    } else {
        Location::new(
            &module.source.source,
            Span {
                start: request.offset,
                end: request.offset,
            },
        )?
    };

    Ok(PositionFacts {
        location,
        targets: if !definitions.is_empty() || available(SemanticCategory::Occurrences) {
            Knowledge::Known(
                definitions
                    .iter()
                    .map(|location| source_identity(*location, SemanticKind::Binding))
                    .collect(),
            )
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        },
        type_information: if input.hover.found {
            Knowledge::Known(type_fact(
                input.hover.text,
                input.hover.is_any,
                &input.type_definitions,
                captured,
            )?)
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        },
        declarations: if !definitions.is_empty() || available(SemanticCategory::Occurrences) {
            Knowledge::Known(definitions.clone())
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        },
        definitions: if !definitions.is_empty() || available(SemanticCategory::Occurrences) {
            Knowledge::Known(definitions)
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        },
        type_definitions: if !type_definitions.is_empty() || available(SemanticCategory::Types) {
            Knowledge::Known(type_definitions)
        } else {
            Knowledge::Unavailable(Reason::UnavailableSemantics)
        },
        documentation: inspection_documentation(
            input.hover.documentation,
            input.hover.found || available(SemanticCategory::Types),
        ),
        deprecated: input.hover.deprecated,
        signatures: signature_information,
        active_signature: input
            .signatures
            .has_active
            .then_some(input.signatures.active),
        active_argument: input.signatures.found.then_some(input.signatures.argument),
        hierarchy: inspection_hierarchy(
            &input.hierarchy,
            &identities,
            available(SemanticCategory::Functions)
                || available(SemanticCategory::Nominals) && input.hierarchy.inheritance_complete,
        ),
    })
}

fn inspection_documentation(hover: String, available: bool) -> Knowledge<Option<String>> {
    if !hover.is_empty() || available {
        Knowledge::Known((!hover.is_empty()).then_some(hover))
    } else {
        Knowledge::Unavailable(Reason::UnavailableSemantics)
    }
}

fn inspection_hierarchy(
    hierarchy: &native::NativeHierarchy,
    identities: &[SemanticIdentity],
    available: bool,
) -> Knowledge<Vec<SemanticIdentity>> {
    if !hierarchy.selected.is_empty() || available {
        Knowledge::Known(
            hierarchy
                .selected
                .iter()
                .map(|selected| identities[*selected].clone())
                .collect(),
        )
    } else {
        Knowledge::Unavailable(Reason::UnavailableSemantics)
    }
}

pub(crate) fn completions_result(
    input: native::NativeCompletions,
    request: &WirePositionInput,
    captured: &Captured,
) -> io::Result<Completions> {
    let module = captured.module(request.analysis.analysis)?;
    let completed = completion(input.completion)?;

    if completed != Completion::Complete && input.suggestions.location.module.is_empty() {
        if !input.suggestions.entries.is_empty() {
            return Err(invalid(
                "native completion candidates omit their replacement source",
            ));
        }

        return Ok(Completions {
            analysis: request.analysis.analysis,
            replacement: Location::new(
                &module.source.source,
                Span {
                    start: request.offset,
                    end: request.offset,
                },
            )?,
            context: CompletionContext::Unknown,
            candidates: Vec::new(),
            operator: None,
            require: false,
            loader: false,
            prefix: None,
            diagnostics: Vec::new(),
            completion: completed,
        });
    }

    let replacement = captured.location(&input.suggestions.location)?;

    if replacement.source != module.source.source.version {
        return Err(invalid(
            "completion replacement addresses a different source",
        ));
    }

    let operator = input
        .suggestions
        .has_index
        .then(|| {
            Location::new(
                &module.source.source,
                Span {
                    start: input.suggestions.index_start,
                    end: input.suggestions.index_end,
                },
            )
        })
        .transpose()?;

    let candidates = input
        .suggestions
        .entries
        .into_iter()
        .map(|suggestion| {
            Ok(CompletionCandidate {
                name: suggestion.name,
                kind: CompletionKind::try_from(suggestion.kind)?,
                detail: suggestion.detail,
                documentation: (!suggestion.documentation.is_empty())
                    .then_some(suggestion.documentation),
                insert_text: (!suggestion.insert_text.is_empty()).then_some(suggestion.insert_text),
                parentheses: Parentheses::try_from(suggestion.parentheses)?,
                deprecated: suggestion.deprecated,
                callable: suggestion.callable,
                preferred: suggestion.preferred,
                notation: Notation::try_from(suggestion.notation)?,
            })
        })
        .collect::<io::Result<_>>()?;

    Ok(Completions {
        analysis: request.analysis.analysis,
        replacement,
        context: CompletionContext::try_from(input.suggestions.context)?,
        candidates,
        operator,
        require: input.suggestions.require,
        loader: input.suggestions.loader,
        prefix: input
            .suggestions
            .has_prefix
            .then_some(input.suggestions.prefix),
        diagnostics: Vec::new(),
        completion: completed,
    })
}

pub(crate) fn lint_result(
    input: native::NativeLintResult,
    request: &WireLintInput,
    captured: &Captured,
) -> io::Result<LintResult> {
    let module = captured.module(request.analysis.analysis)?;
    let completed = completion(input.completion)?;
    let reason = completion_reason(&completed).unwrap_or(Reason::UnavailableSemantics);

    let selected = request
        .warnings
        .enabled
        .iter()
        .map(|warning| warning.name.as_str())
        .collect::<BTreeSet<_>>();

    let mut observed = BTreeSet::new();

    let warnings = input
        .availability
        .into_iter()
        .map(|warning| {
            if !selected.contains(warning.name.as_str()) || !observed.insert(warning.name.clone()) {
                return Err(invalid(
                    "native warning availability is duplicated or unrequested",
                ));
            }

            Ok(WarningAvailability {
                warning: UpstreamWarningIdentity { name: warning.name },
                status: match warning.status {
                    0 => {
                        if matches!(completed, Completion::Cancelled | Completion::TimedOut) {
                            Availability::Unavailable(reason)
                        } else {
                            Availability::Available
                        }
                    }

                    1 => Availability::Partial(reason),
                    2 => Availability::Unavailable(reason),
                    _ => return Err(invalid("unknown native warning availability")),
                },
            })
        })
        .collect::<io::Result<Vec<_>>>()?;

    if observed.len() != selected.len() {
        return Err(invalid(
            "native warning availability omits a requested warning",
        ));
    }

    let finding = |warning: native::NativeWarning| {
        if warning.code <= 0 || !selected.contains(warning.name.as_str()) {
            return Err(invalid("native warning has an invalid code or identity"));
        }

        let primary = captured.location(&warning.location)?;

        if primary.source != module.source.source.version {
            return Err(invalid("native warning addresses a different source"));
        }

        let identity = UpstreamWarningIdentity { name: warning.name };

        let tags = match identity.name.as_str() {
            "LocalUnused" | "FunctionUnused" | "ImportUnused" | "UnreachableCode" => {
                vec![DiagnosticTag::Unnecessary]
            }

            "DeprecatedGlobal" | "DeprecatedApi" => vec![DiagnosticTag::Deprecated],
            _ => Vec::new(),
        };

        Ok(UpstreamFinding {
            warning: identity.clone(),
            diagnostic: Diagnostic {
                identity: FindingIdentity::new(
                    "luau",
                    captured.inputs(),
                    warning.code.to_string(),
                    primary,
                ),
                code: identity.name,
                category: DiagnosticCategory::Lint,
                severity: Severity::Warning,
                message: warning.message,
                primary,
                related: Vec::new(),
                tags,
            },
        })
    };

    let findings = input
        .warnings
        .into_iter()
        .map(finding)
        .collect::<io::Result<_>>()?;

    let suppression_findings = input
        .suppression_warnings
        .into_iter()
        .map(|warning| finding(warning).map(|finding| finding.diagnostic))
        .collect::<io::Result<_>>()?;

    Ok(LintResult {
        source: module.source.source.version,
        findings,
        suppression_findings,
        prerequisite_diagnostics: input
            .diagnostics
            .iter()
            .map(|diagnostic_input| diagnostic(diagnostic_input, captured))
            .collect::<io::Result<_>>()?,
        warnings,
        completion: completed,
    })
}

pub(crate) fn validate_configuration(input: &ConfigurationInput) -> io::Result<()> {
    input.source.validate()?;
    flags::normalize(&input.language.flags)?;

    if input.context.base.contains('\0')
        || input.limits.memory_bytes < 1024 * 1024
        || usize::try_from(input.limits.memory_bytes).is_err()
        || input.limits.extraction_depth == 0
        || input.limits.extraction_depth > 128
        || input.limits.extraction_entries == 0
        || input.limits.extraction_entries > 100_000
    {
        return Err(invalid("invalid configuration context or resource limits"));
    }

    Ok(())
}

pub(crate) fn configuration_input(
    input: &ConfigurationInput,
) -> io::Result<native::NativeConfigurationInput> {
    let mut nodes = Vec::new();

    if !input.context.executable {
        let value = serde_json::from_str::<Value>(&input.source.text).map_err(invalid)?;
        let mut entries = 0;
        flatten_configuration(&value, &mut nodes, &mut entries, 1, &input.limits)?;
    }

    Ok(native::NativeConfigurationInput {
        text: input.source.text.to_string(),
        base: input.context.base.clone(),
        executable: input.context.executable,
        memory_bytes: input.limits.memory_bytes,
        extraction_depth: input.limits.extraction_depth,
        extraction_entries: input.limits.extraction_entries,
        root: 0,
        nodes,
    })
}

fn flatten_configuration(
    value: &Value,
    nodes: &mut Vec<native::NativeConfigurationNode>,
    count: &mut u32,
    depth: u32,
    limits: &ConfigurationLimits,
) -> io::Result<u32> {
    if depth > limits.extraction_depth {
        return Err(Reason::AnalysisLimit.error());
    }

    let index = u32::try_from(nodes.len()).map_err(invalid)?;

    nodes.push(native::NativeConfigurationNode {
        kind: 3,
        text: String::new(),
        number: 0.0,
        boolean: false,
        entries: Vec::new(),
    });

    let mut node = native::NativeConfigurationNode {
        kind: 3,
        text: String::new(),
        number: 0.0,
        boolean: false,
        entries: Vec::new(),
    };

    match value {
        Value::String(value) => {
            node.kind = 0;
            node.text.clone_from(value);
        }

        Value::Number(value) => {
            node.kind = 1;

            node.number = value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| invalid("configuration numbers must be finite"))?;
        }

        Value::Bool(value) => {
            node.kind = 2;
            node.boolean = *value;
        }

        Value::Object(values) => {
            for (key, value) in values {
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| Reason::AnalysisLimit.error())?;

                if *count > limits.extraction_entries {
                    return Err(Reason::AnalysisLimit.error());
                }

                let value_index = flatten_configuration(value, nodes, count, depth + 1, limits)?;

                node.entries.push(native::NativeConfigurationEntry {
                    key_kind: 0,
                    key_text: key.clone(),
                    key_number: 0.0,
                    value_index,
                });
            }
        }

        Value::Array(values) => {
            for (position, value) in values.iter().enumerate() {
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| Reason::AnalysisLimit.error())?;

                if *count > limits.extraction_entries {
                    return Err(Reason::AnalysisLimit.error());
                }

                let value_index = flatten_configuration(value, nodes, count, depth + 1, limits)?;

                node.entries.push(native::NativeConfigurationEntry {
                    key_kind: 1,
                    key_text: String::new(),
                    key_number: f64::from(u32::try_from(position + 1).map_err(invalid)?),
                    value_index,
                });
            }
        }

        Value::Null => {
            return Err(invalid(
                "configuration values must be strings, numbers, booleans or tables",
            ));
        }
    }

    nodes[index as usize] = node;

    Ok(index)
}

pub(crate) fn configuration_failure(
    input: &ConfigurationInput,
    message: String,
    completed: Completion,
) -> io::Result<ConfigurationResult> {
    let primary = Location::new(
        &input.source,
        Span {
            start: 0,
            end: input.source.text.len(),
        },
    )?;

    let code = "configuration".to_owned();

    Ok(ConfigurationResult {
        source: input.source.version,
        values: None,
        diagnostics: vec![Diagnostic {
            identity: FindingIdentity::new(
                "luau.configuration",
                vec![input.source.version],
                &code,
                primary,
            ),
            code,
            category: DiagnosticCategory::Configuration,
            severity: Severity::Error,
            message,
            primary,
            related: Vec::new(),
            tags: Vec::new(),
        }],
        completion: completed,
    })
}

pub(crate) fn configuration_result(
    output: &native::NativeConfigurationResult,
    input: &ConfigurationInput,
) -> io::Result<ConfigurationResult> {
    let completed = completion(output.completion)?;

    if !output.has_values {
        if !output.nodes.is_empty() || matches!(completed, Completion::Complete) {
            return Err(invalid(
                "native configuration omitted values without an incomplete outcome",
            ));
        }

        return configuration_failure(input, output.message.clone(), completed);
    }

    if !matches!(completed, Completion::Complete)
        || !output.message.is_empty()
        || output.nodes.len() > input.limits.extraction_entries as usize + 1
    {
        return Err(invalid(
            "native configuration values have inconsistent completion or bounds",
        ));
    }

    let mut visited = BTreeSet::new();
    let mut entries = 0;

    let values = extract_configuration(
        output.root,
        &output.nodes,
        &mut visited,
        &mut entries,
        1,
        &input.limits,
    )?;

    if visited.len() != output.nodes.len() || !matches!(values, ConfigurationValue::Table(_)) {
        return Err(invalid(
            "native configuration contains unreachable values or omits its root table",
        ));
    }

    Ok(ConfigurationResult {
        source: input.source.version,
        values: Some(values),
        diagnostics: Vec::new(),
        completion: completed,
    })
}

fn extract_configuration(
    index: u32,
    nodes: &[native::NativeConfigurationNode],
    visited: &mut BTreeSet<u32>,
    count: &mut u32,
    depth: u32,
    limits: &ConfigurationLimits,
) -> io::Result<ConfigurationValue> {
    if depth > limits.extraction_depth || !visited.insert(index) {
        return Err(invalid(
            "native configuration contains a cycle, shared node or excessive depth",
        ));
    }

    let node = nodes
        .get(index as usize)
        .ok_or_else(|| invalid("native configuration value index is invalid"))?;

    Ok(match node.kind {
        0 if node.entries.is_empty() => ConfigurationValue::String(node.text.clone()),

        1 if node.entries.is_empty() && node.number.is_finite() => {
            ConfigurationValue::Number(node.number)
        }

        2 if node.entries.is_empty() => ConfigurationValue::Boolean(node.boolean),

        3 => {
            let mut keys = BTreeSet::new();
            let mut values = Vec::with_capacity(node.entries.len());

            for entry in &node.entries {
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| invalid("native configuration entry count exceeds limits"))?;

                if *count > limits.extraction_entries {
                    return Err(invalid("native configuration entry count exceeds limits"));
                }

                let key = match entry.key_kind {
                    0 if keys.insert((0, entry.key_text.clone(), 0)) => {
                        ConfigurationKey::String(entry.key_text.clone())
                    }

                    1 if entry.key_number.is_finite()
                        && keys.insert((
                            1,
                            String::new(),
                            if entry.key_number == 0.0 {
                                0
                            } else {
                                entry.key_number.to_bits()
                            },
                        )) =>
                    {
                        ConfigurationKey::Number(entry.key_number)
                    }

                    _ => {
                        return Err(invalid(
                            "native configuration key is duplicated, nonfinite or unsupported",
                        ));
                    }
                };

                let value = extract_configuration(
                    entry.value_index,
                    nodes,
                    visited,
                    count,
                    depth + 1,
                    limits,
                )?;

                values.push(ConfigurationEntry { key, value });
            }

            values.sort_by(|left, right| match (&left.key, &right.key) {
                (ConfigurationKey::String(left), ConfigurationKey::String(right)) => {
                    left.cmp(right)
                }

                (ConfigurationKey::Number(left), ConfigurationKey::Number(right)) => {
                    left.total_cmp(right)
                }

                (ConfigurationKey::Number(_), ConfigurationKey::String(_)) => Ordering::Less,
                (ConfigurationKey::String(_), ConfigurationKey::Number(_)) => Ordering::Greater,
            });

            ConfigurationValue::Table(values)
        }

        _ => {
            return Err(invalid(
                "native configuration value tag or payload is invalid",
            ));
        }
    })
}

pub(crate) fn configuration_response(
    output: native::NativeConfigurationResult,
    input: &ConfigurationInput,
    identities: (u64, u64),
) -> io::Result<ConfigurationResponse> {
    let result = configuration_result(&output, input)?;

    Ok(ConfigurationResponse {
        request: identities.0,
        generation: identities.1,
        source: result.source,
        root: result.values.is_some().then_some(output.root),
        nodes: output
            .nodes
            .into_iter()
            .map(|node| Node {
                kind: node.kind,
                text: node.text,
                number: node.number,
                boolean: node.boolean,
                entries: node
                    .entries
                    .into_iter()
                    .map(|entry| Entry {
                        key_kind: entry.key_kind,
                        key_text: entry.key_text,
                        key_number: entry.key_number,
                        value_index: entry.value_index,
                    })
                    .collect(),
            })
            .collect(),
        diagnostics: result.diagnostics,
        completion: result.completion,
    })
}

pub(crate) fn configuration_wire_result(
    output: ConfigurationResponse,
    input: &ConfigurationInput,
) -> io::Result<ConfigurationResult> {
    let captured = Captured::configuration(&input.source)?;

    for diagnostic in &output.diagnostics {
        validate_diagnostic(diagnostic, &captured)?;
    }

    validate_completion(&output.completion)?;

    if output.source != input.source.version {
        return Err(invalid("configuration result addresses a different source"));
    }

    let values = if let Some(root) = output.root {
        if !matches!(output.completion, Completion::Complete)
            || output.nodes.len() > input.limits.extraction_entries as usize + 1
        {
            return Err(invalid(
                "configuration values have inconsistent completion or bounds",
            ));
        }

        let nodes = output
            .nodes
            .into_iter()
            .map(|node| native::NativeConfigurationNode {
                kind: node.kind,
                text: node.text,
                number: node.number,
                boolean: node.boolean,
                entries: node
                    .entries
                    .into_iter()
                    .map(|entry| native::NativeConfigurationEntry {
                        key_kind: entry.key_kind,
                        key_text: entry.key_text,
                        key_number: entry.key_number,
                        value_index: entry.value_index,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();

        let mut visited = BTreeSet::new();
        let mut entries = 0;

        let values =
            extract_configuration(root, &nodes, &mut visited, &mut entries, 1, &input.limits)?;

        if visited.len() != nodes.len() || !matches!(values, ConfigurationValue::Table(_)) {
            return Err(invalid(
                "configuration result contains unreachable values or omits its root table",
            ));
        }

        Some(values)
    } else {
        if matches!(output.completion, Completion::Complete) || !output.nodes.is_empty() {
            return Err(invalid(
                "configuration result omitted values without an incomplete outcome",
            ));
        }

        None
    };

    Ok(ConfigurationResult {
        source: output.source,
        values,
        diagnostics: output.diagnostics,
        completion: output.completion,
    })
}

fn validate_completion(completion: &Completion) -> io::Result<()> {
    if matches!(completion, Completion::Incomplete(reasons) if reasons.is_empty()) {
        return Err(invalid(
            "incomplete native result omits its coverage reasons",
        ));
    }

    Ok(())
}

pub(crate) fn validate_diagnostic(diagnostic: &Diagnostic, captured: &Captured) -> io::Result<()> {
    captured.validate_location(diagnostic.primary)?;

    for related in &diagnostic.related {
        captured.validate_location(related.location)?;
    }

    let code = if diagnostic.category == DiagnosticCategory::Lint {
        let original = diagnostic.identity.code();

        if !original
            .parse::<i32>()
            .is_ok_and(|code| code > 0 && code.to_string() == original)
        {
            return Err(invalid(
                "native lint finding has an invalid original warning code",
            ));
        }

        original
    } else {
        &diagnostic.code
    };

    if diagnostic.identity
        != FindingIdentity::new(
            if captured.analysis.is_some() {
                "luau"
            } else {
                "luau.configuration"
            },
            captured.inputs(),
            code,
            diagnostic.primary,
        )
    {
        return Err(invalid(
            "native finding identity disagrees with its exact captured inputs or primary location",
        ));
    }

    Ok(())
}

pub(crate) fn validate_analysis_result(
    output: &WireAnalysisResult,
    input: &AnalysisRequest,
    captured: &Captured,
) -> io::Result<()> {
    validate_completion(&output.completion)?;

    if output.closure != input.closure
        || output.frontend_revision == 0
        || output.modules.len() != input.modules.len()
    {
        return Err(invalid(
            "native analysis result omitted or changed its closure, revision or modules",
        ));
    }

    for (output, module) in output.modules.iter().zip(&input.modules) {
        if output.analysis != module.analysis
            || output.parsed.input != module.source
            || output.parsed.imports
                != module
                    .imports
                    .iter()
                    .map(|import| import.site.clone())
                    .collect::<Vec<_>>()
            || output
                .environment
                .as_ref()
                .map(|environment| &environment.input)
                != module.environment.as_ref()
        {
            return Err(invalid(
                "native module result disagrees with its exact captured inputs",
            ));
        }

        validate_completion(&output.completion)?;
        validate_completion(&output.parsed.completion)?;

        for directive in &output.parsed.directives {
            directive.location.validate(&module.source.source)?;
        }

        for diagnostic in output.diagnostics.iter().chain(&output.parsed.diagnostics) {
            validate_diagnostic(diagnostic, captured)?;
        }

        if let Some(environment) = &output.environment {
            validate_completion(&environment.completion)?;

            for diagnostic in &environment.diagnostics {
                validate_diagnostic(diagnostic, captured)?;
            }
        }

        if output
            .availability
            .categories
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != module.inference.retained.iter().copied().collect()
        {
            return Err(invalid(
                "native analysis category availability omits or changes the request",
            ));
        }
    }

    Ok(())
}

fn validate_identity(identity: &SemanticIdentity, captured: &Captured) -> io::Result<()> {
    match identity {
        SemanticIdentity::Source { location, .. } => captured.validate_location(*location),

        SemanticIdentity::Environment {
            environment,
            namespace,
            name,
            ..
        } if !name.is_empty()
            && !name.contains('\0')
            && captured.modules.values().any(|module| {
                module.environment.as_ref().is_some_and(|input| {
                    input.identity == *environment
                        && (namespace == "@luau"
                            || input
                                .declarations
                                .iter()
                                .flat_map(|group| &group.definitions)
                                .any(|definition| definition.namespace == *namespace)
                            || namespace.is_empty() && input.globals.contains(name))
                })
            }) =>
        {
            Ok(())
        }

        SemanticIdentity::Environment { .. } => {
            Err(invalid("semantic environment identity is not captured"))
        }
    }
}

fn validate_type(information: &Knowledge<TypeFact>, captured: &Captured) -> io::Result<()> {
    if let Knowledge::Known(information) = information {
        if information.display.is_empty() {
            return Err(invalid("established native type has empty display text"));
        }

        for declaration in &information.declarations {
            validate_identity(declaration, captured)?;
        }
    }

    Ok(())
}

fn validate_signatures(
    information: &Knowledge<Vec<CallableSignature>>,
    captured: &Captured,
) -> io::Result<()> {
    if let Knowledge::Known(signatures) = information {
        for signature in signatures {
            if signature.label.is_empty() {
                return Err(invalid("established signature has an empty label"));
            }

            let mut end = 0;

            for parameter in &signature.parameters {
                if parameter.label_range[0] < end
                    || parameter.label_range[0] >= parameter.label_range[1]
                    || !signature.label.is_char_boundary(parameter.label_range[0])
                    || !signature.label.is_char_boundary(parameter.label_range[1])
                {
                    return Err(invalid("signature parameter label range is invalid"));
                }

                end = parameter.label_range[1];

                if let Some(declaration) = parameter.declaration {
                    captured.validate_location(declaration)?;
                }
            }

            if let Knowledge::Known(minimum) = signature.minimum_arguments
                && minimum > signature.parameters.len()
            {
                return Err(invalid("signature minimum exceeds its parameter count"));
            }
        }
    }

    Ok(())
}

fn validate_effects(information: &Knowledge<EffectFact>, captured: &Captured) -> io::Result<()> {
    if let Knowledge::Known(information) = information {
        for identity in information.reads.iter().chain(&information.writes) {
            validate_identity(identity, captured)?;
        }
    }

    Ok(())
}

pub(crate) fn validate_facts_result(
    output: &FactsResult,
    request: &WireFactsInput,
    captured: &Captured,
) -> io::Result<()> {
    let module = captured.module(request.analysis.analysis)?;

    if output.analysis != request.analysis.analysis
        || output
            .availability
            .categories
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != request.categories.iter().copied().collect()
    {
        return Err(invalid(
            "native facts identity or category coverage disagrees with the request",
        ));
    }

    validate_completion(&output.completion)?;

    if matches!(output.completion, Completion::Complete)
        && output
            .availability
            .categories
            .values()
            .any(|status| !matches!(status, Availability::Available))
    {
        return Err(invalid("complete native facts omit requested coverage"));
    }

    for diagnostic in &output.diagnostics {
        validate_diagnostic(diagnostic, captured)?;
    }

    for binding in &output.facts.bindings {
        validate_identity(&binding.identity, captured)?;

        if let Knowledge::Known(Some(declaration)) = binding.declaration {
            captured.validate_location(declaration)?;
        }

        validate_identity(&binding.scope, captured)?;

        if let Knowledge::Known(Some(shadows)) = &binding.shadows {
            validate_identity(shadows, captured)?;
        }

        validate_type(&binding.type_information, captured)?;
    }

    for occurrence in &output.facts.occurrences {
        captured.validate_location(occurrence.location)?;
        occurrence.location.validate(&module.source.source)?;

        if let Knowledge::Known(targets) = &occurrence.targets {
            for target in targets {
                validate_identity(target, captured)?;
            }
        }
    }

    for expression in &output.facts.expressions {
        captured.validate_location(expression.location)?;
        expression.location.validate(&module.source.source)?;
        validate_type(&expression.type_information, captured)?;

        if matches!(expression.constant, Knowledge::Known(Some(ConstantValue::Number(value))) if !value.is_finite())
        {
            return Err(invalid("native constant number is nonfinite"));
        }

        if let Knowledge::Known(dependencies) = &expression.dependencies {
            for dependency in dependencies {
                validate_identity(dependency, captured)?;
            }
        }

        validate_effects(&expression.effects, captured)?;
    }

    for call in &output.facts.calls {
        captured.validate_location(call.location)?;
        captured.validate_location(call.callee)?;

        if !contains(call.location, call.callee) {
            return Err(invalid("native callee escapes its call"));
        }

        validate_identity(&call.caller, captured)?;

        if let Knowledge::Known(targets) = &call.targets {
            for target in targets {
                validate_identity(target, captured)?;
            }
        }

        validate_signatures(&call.signatures, captured)?;

        for argument in &call.arguments {
            captured.validate_location(argument.argument)?;

            if !contains(call.location, argument.argument) {
                return Err(invalid("native argument escapes its call"));
            }

            if let Knowledge::Known(Some(parameter)) = argument.parameter
                && !matches!(&call.signatures, Knowledge::Known(signatures) if signatures.iter().any(|signature| parameter < signature.parameters.len()))
            {
                return Err(invalid("native argument parameter is not established"));
            }
        }

        validate_effects(&call.effects, captured)?;
    }

    validate_structural_facts(&output.facts, captured)
}

fn validate_structural_facts(facts: &SemanticFacts, captured: &Captured) -> io::Result<()> {
    for scope in &facts.scopes {
        validate_identity(&scope.identity, captured)?;
        captured.validate_location(scope.location)?;

        if let Some(parent) = &scope.parent {
            validate_identity(parent, captured)?;
        }
    }

    for function in &facts.functions {
        validate_identity(&function.identity, captured)?;

        for location in [function.location, function.selection, function.body] {
            captured.validate_location(location)?;
        }

        if !contains(function.location, function.selection)
            || !contains(function.location, function.body)
        {
            return Err(invalid(
                "native function selection or body escapes its declaration",
            ));
        }

        validate_identity(&function.scope, captured)?;
        validate_signatures(&function.signatures, captured)?;
    }

    for nominal in &facts.nominals {
        validate_identity(&nominal.identity, captured)?;
        captured.validate_location(nominal.declaration)?;
        captured.validate_location(nominal.selection)?;

        if !contains(nominal.declaration, nominal.selection) {
            return Err(invalid("native nominal selection escapes its declaration"));
        }
    }

    for inheritance in &facts.inheritance {
        validate_identity(&inheritance.child, captured)?;
        validate_identity(&inheritance.parent, captured)?;
    }

    Ok(())
}

pub(crate) fn validate_inspection_result(
    output: &InspectionResult,
    request: &WirePositionInput,
    captured: &Captured,
) -> io::Result<()> {
    let module = captured.module(request.analysis.analysis)?;

    if output.analysis != request.analysis.analysis
        || output
            .availability
            .categories
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != module.inference.retained.iter().copied().collect()
    {
        return Err(invalid(
            "native inspection identity or coverage disagrees with its established state",
        ));
    }

    validate_completion(&output.completion)?;

    for diagnostic in &output.diagnostics {
        validate_diagnostic(diagnostic, captured)?;
    }

    if let Some(occurrence) = &output.occurrence {
        captured.validate_location(occurrence.location)?;

        if occurrence.location.source != module.source.source.version
            || occurrence.location.span.start > request.offset
            || occurrence.location.span.end < request.offset
        {
            return Err(invalid(
                "native inspection range does not address its requested position",
            ));
        }

        if let Knowledge::Known(targets) = &occurrence.targets {
            for target in targets {
                validate_identity(target, captured)?;
            }
        }

        validate_type(&occurrence.type_information, captured)?;

        for locations in [
            &occurrence.declarations,
            &occurrence.definitions,
            &occurrence.type_definitions,
        ] {
            if let Knowledge::Known(locations) = locations {
                for location in locations {
                    captured.validate_location(*location)?;
                }
            }
        }

        validate_signatures(&occurrence.signatures, captured)?;

        if let Knowledge::Known(signatures) = &occurrence.signatures
            && occurrence
                .active_signature
                .is_some_and(|active| active >= signatures.len())
        {
            return Err(invalid(
                "inspection active signature is outside its candidates",
            ));
        }

        if let Knowledge::Known(identities) = &occurrence.hierarchy {
            for identity in identities {
                validate_identity(identity, captured)?;
            }
        }
    }

    Ok(())
}

pub(crate) fn validate_lint_result(
    output: &LintResult,
    request: &WireLintInput,
    captured: &Captured,
) -> io::Result<()> {
    let module = captured.module(request.analysis.analysis)?;
    validate_completion(&output.completion)?;
    let selected = request.warnings.enabled.iter().collect::<BTreeSet<_>>();

    if output.source != module.source.source.version
        || output.warnings.len() != selected.len()
        || output
            .warnings
            .iter()
            .map(|warning| &warning.warning)
            .collect::<BTreeSet<_>>()
            != selected
    {
        return Err(invalid(
            "native lint source or warning coverage disagrees with the request",
        ));
    }

    for finding in &output.findings {
        if !selected.contains(&finding.warning)
            || finding.diagnostic.code != finding.warning.name
            || finding.diagnostic.category != DiagnosticCategory::Lint
            || finding.diagnostic.primary.source != output.source
        {
            return Err(invalid(
                "native lint finding has an inconsistent warning identity or source",
            ));
        }
    }

    for diagnostic in output
        .findings
        .iter()
        .map(|finding| &finding.diagnostic)
        .chain(&output.suppression_findings)
    {
        if !selected
            .iter()
            .any(|warning| warning.name == diagnostic.code)
            || diagnostic.category != DiagnosticCategory::Lint
            || diagnostic.severity != Severity::Warning
            || diagnostic.primary.source != output.source
        {
            return Err(invalid(
                "native lint finding has an inconsistent warning identity or source",
            ));
        }

        validate_diagnostic(diagnostic, captured)?;
    }

    for diagnostic in &output.prerequisite_diagnostics {
        validate_diagnostic(diagnostic, captured)?;
    }

    if matches!(output.completion, Completion::Complete)
        && output
            .warnings
            .iter()
            .any(|warning| !matches!(warning.status, Availability::Available))
    {
        return Err(invalid(
            "complete native lint omitted requested warning coverage",
        ));
    }

    Ok(())
}

pub(crate) fn validate_completions_result(
    output: &Completions,
    request: &WirePositionInput,
    captured: &Captured,
) -> io::Result<()> {
    let module = captured.module(request.analysis.analysis)?;
    validate_completion(&output.completion)?;
    output.replacement.validate(&module.source.source)?;

    if output.analysis != request.analysis.analysis
        || output.replacement.span.start > request.offset
        || output.replacement.span.end < request.offset
    {
        return Err(invalid(
            "native completion does not address its requested position",
        ));
    }

    if let Some(operator) = output.operator {
        operator.validate(&module.source.source)?;

        if operator.span.end > output.replacement.span.start {
            return Err(invalid(
                "native completion operator overlaps its replacement",
            ));
        }
    }

    if output
        .candidates
        .iter()
        .any(|candidate| candidate.name.is_empty())
        || output
            .candidates
            .windows(2)
            .any(|pair| pair[0].name >= pair[1].name)
    {
        return Err(invalid(
            "native completion candidates are empty, duplicated or unordered",
        ));
    }

    if (output.require || output.prefix.is_some()) && output.context != CompletionContext::String {
        return Err(invalid("native completion import context is inconsistent"));
    }

    for diagnostic in &output.diagnostics {
        validate_diagnostic(diagnostic, captured)?;
    }

    Ok(())
}

fn contains(container: Location, region: Location) -> bool {
    container.source == region.source
        && container.span.start <= region.span.start
        && container.span.end >= region.span.end
}
