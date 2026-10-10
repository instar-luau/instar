//! Typed native flags and worker lifetime contracts.

use std::{collections::BTreeMap, io, time::Duration};

use instar_luau::{
    FlagValue, ImportIdentity, ImportOutcome, ImportRequest, ImportSite, LanguageProfile, Native,
    ResolvedImport, flags,
};

use instar_shared::DiagnosticCategory;

use crate::support::{location, module, options, request};

#[test]
fn flags_validate_registry_names_types_and_aliases() {
    for (name, value) in [
        ("MissingInstarFlag", FlagValue::Boolean(true)),
        ("LuauRecursionLimit", FlagValue::Boolean(true)),
        ("LuauExportValueSyntax", FlagValue::Integer(1)),
    ] {
        assert_eq!(
            flags::normalize(&BTreeMap::from([(name.to_owned(), value)]))
                .expect_err("invalid flag")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    assert!(
        flags::normalize(&BTreeMap::from([
            ("LuauExportValueSyntax".to_owned(), FlagValue::Boolean(true)),
            (
                "LuauExportValueSyntax5".to_owned(),
                FlagValue::Boolean(true)
            )
        ]))
        .is_err()
    );
}

#[test]
fn stdout_flags_reject_enabling_and_accept_disabling() {
    for name in [
        "DebugLuauLogSolver",
        "DebugLuauLogBindings",
        "DebugLuauLogSolverToJson",
        "DebugLuauLogCFG",
        "DebugLuauDumpCFGJson",
        "DebugLuauLogTypeFamilies",
        "DebugLuauMagicTypes",
    ] {
        let error = flags::normalize(&BTreeMap::from([(
            name.to_owned(),
            FlagValue::Boolean(true),
        )]))
        .expect_err("stdout flag must fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        assert_eq!(
            error.to_string(),
            format!("Luau flag writes to stdout and is unavailable: {name}")
        );

        flags::normalize(&BTreeMap::from([(
            name.to_owned(),
            FlagValue::Boolean(false),
        )]))
        .expect("disabling stdout must remain valid");
    }
}

#[test]
fn integer_changes_restart_cached_analysis_and_defaults_are_restored() -> io::Result<()> {
    let source = format!("return {}1{}", "(".repeat(40), ")".repeat(40));
    let mut input = request(vec![module(1, &source, 1)?])?;
    let mut native = Native::default();

    assert_eq!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))?
            .modules[0]
            .diagnostics
            .as_slice(),
        []
    );

    let changed = BTreeMap::from([("LuauRecursionLimit".to_owned(), FlagValue::Integer(20))]);
    input.profile = LanguageProfile::new(&changed, 64 * 1024 * 1024)?;
    input.modules[0].analysis.revision = 2;
    input.modules[0].source.language.flags = changed.clone();

    input.modules[0]
        .environment
        .as_mut()
        .expect("environment")
        .language
        .flags = changed;

    assert!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))?
            .modules[0]
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Syntax)
    );

    input.profile = LanguageProfile::new(&BTreeMap::new(), 64 * 1024 * 1024)?;
    input.modules[0].analysis.revision = 3;
    input.modules[0].source.language.flags.clear();

    input.modules[0]
        .environment
        .as_mut()
        .expect("environment")
        .language
        .flags
        .clear();

    assert_eq!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))?
            .modules[0]
            .diagnostics
            .as_slice(),
        []
    );

    native.finish()
}

#[test]
fn reachable_modules_require_consistent_effective_flags() -> io::Result<()> {
    let source = "return require('dependency')";
    let mut entry = module(1, source, 1)?;
    let dependency = module(2, "return 1", 1)?;

    entry.imports.push(ResolvedImport {
        site: ImportSite {
            identity: ImportIdentity(1),
            call: location(&entry, [7, source.len()])?,
            argument: location(&entry, [15, source.len() - 1])?,
            request: ImportRequest::Static("dependency".to_owned()),
        },
        outcome: ImportOutcome::Module(dependency.analysis.module),
    });

    let mut input = request(vec![entry, dependency])?;
    input.roots.truncate(1);
    let mut native = Native::default();

    assert_eq!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))?
            .modules[0]
            .diagnostics
            .as_slice(),
        []
    );

    let changed = BTreeMap::from([("LuauRecursionLimit".to_owned(), FlagValue::Integer(20))]);
    input.profile = LanguageProfile::new(&changed, 64 * 1024 * 1024)?;
    input.modules[0].analysis.revision = 2;
    input.modules[0].source.language.flags = changed.clone();

    input.modules[0]
        .environment
        .as_mut()
        .expect("environment")
        .language
        .flags = changed.clone();

    assert!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))
            .expect_err("conflicting reachable flags")
            .to_string()
            .contains("disagrees")
    );

    input.modules[1].analysis.revision = 2;
    input.modules[1].source.language.flags = changed.clone();

    input.modules[1]
        .environment
        .as_mut()
        .expect("environment")
        .language
        .flags = changed;

    assert_eq!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))?
            .modules[0]
            .diagnostics
            .as_slice(),
        []
    );

    let canonical =
        BTreeMap::from([("LuauExportValueSyntax".to_owned(), FlagValue::Boolean(true))]);

    let alias = BTreeMap::from([(
        "LuauExportValueSyntax5".to_owned(),
        FlagValue::Boolean(true),
    )]);

    input.profile = LanguageProfile::new(&canonical, 64 * 1024 * 1024)?;

    for (module, flags) in input.modules.iter_mut().zip([canonical, alias]) {
        module.analysis.revision = 3;
        module.source.language.flags = flags.clone();

        module
            .environment
            .as_mut()
            .expect("environment")
            .language
            .flags = flags;
    }

    assert_eq!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))?
            .modules[0]
            .diagnostics
            .as_slice(),
        []
    );

    native.finish()
}
