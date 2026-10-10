use std::{
    collections::BTreeMap,
    io,
    time::{Duration, Instant},
};

use instar_luau::{
    AnalysisIdentity, AnalysisPurpose, AnalysisRequest, Capabilities, ClosureIdentity,
    ConfigurationContext, ConfigurationInput, ConfigurationLimits, DeclarationGroup, Definition,
    EnvironmentIdentity, EnvironmentInput, InferenceMode, InferenceSettings, LanguageFeatures,
    LanguageProfile, ModuleIdentity, ModuleInput, ReportingSettings, SemanticCategory, SourceInput,
};

use instar_shared::{
    ContextIdentity, Location, Options, SourceIdentity, SourceSnapshot, SourceVersion, Span,
};

pub(crate) fn options(duration: Duration) -> Options {
    Options::new(Some(Instant::now() + duration))
}

pub(crate) fn module(identity: u64, text: &str, revision: u64) -> io::Result<ModuleInput> {
    let module = ModuleIdentity {
        context: ContextIdentity::new(1),
        source: SourceIdentity::new(identity),
    };

    let source = SourceSnapshot::new(
        SourceVersion {
            identity: module.source,
            revision,
        },
        text,
    )?;

    Ok(ModuleInput {
        analysis: AnalysisIdentity { module, revision },
        source: SourceInput {
            module,
            source,
            language: LanguageFeatures::default(),
        },
        environment: Some(EnvironmentInput {
            identity: EnvironmentIdentity::new(identity),
            language: LanguageFeatures::default(),
            globals: Vec::new(),
            declarations: vec![DeclarationGroup::default()],
            capabilities: Capabilities::default(),
            instances: None,
        }),
        inference: InferenceSettings {
            required: true,
            mode: if text.lines().next() == Some("--!strict") {
                InferenceMode::Strict
            } else {
                InferenceMode::Nonstrict
            },
            retained: vec![
                SemanticCategory::Bindings,
                SemanticCategory::Occurrences,
                SemanticCategory::Types,
                SemanticCategory::Scopes,
                SemanticCategory::Functions,
                SemanticCategory::Nominals,
            ],
        },
        reporting: ReportingSettings {
            ordinary_type_diagnostics: true,
        },
        imports: Vec::new(),
    })
}

pub(crate) fn request(mut modules: Vec<ModuleInput>) -> io::Result<AnalysisRequest> {
    modules.sort_by_key(|module| module.analysis.module);

    Ok(AnalysisRequest {
        closure: ClosureIdentity::new(1),
        profile: LanguageProfile::new(&BTreeMap::new(), 64 * 1024 * 1024)?,
        purpose: AnalysisPurpose::Source,
        roots: modules
            .iter()
            .map(|module| module.analysis.module)
            .collect(),
        modules,
        cycles: Vec::new(),
    })
}

pub(crate) fn location(module: &ModuleInput, range: [usize; 2]) -> io::Result<Location> {
    Location::new(
        &module.source.source,
        Span {
            start: range[0],
            end: range[1],
        },
    )
}

pub(crate) fn definition(
    identity: u64,
    text: &str,
    revision: u64,
    namespace: &str,
) -> io::Result<Definition> {
    Ok(Definition {
        source: SourceSnapshot::new(
            SourceVersion {
                identity: SourceIdentity::new(identity),
                revision,
            },
            text,
        )?,
        namespace: namespace.to_owned(),
    })
}

pub(crate) fn configuration(text: &str, executable: bool) -> io::Result<ConfigurationInput> {
    Ok(ConfigurationInput {
        source: SourceSnapshot::new(
            SourceVersion {
                identity: SourceIdentity::new(1),
                revision: 1,
            },
            text,
        )?,
        language: LanguageFeatures::default(),
        context: ConfigurationContext {
            identity: ContextIdentity::new(1),
            base: String::from("."),
            executable,
        },
        limits: ConfigurationLimits {
            memory_bytes: 64 * 1024 * 1024,
            extraction_depth: 128,
            extraction_entries: 100_000,
        },
    })
}
