//! Native documentation namespace and declaration identity contracts.

use std::{io, time::Duration};

use instar_luau::{
    AnalysisReference, AnalysisRequest, AnalysisResult, Definition, ModuleInput, Native,
    PositionInput,
};

use instar_shared::{Availability, Completion, Knowledge, Reason};

use crate::support::{definition, module, options, request};

fn namespace_fixture(source: &str) -> io::Result<(ModuleInput, Vec<Definition>)> {
    let mut entry = module(1, source, 1)?;

    let definitions = vec![
        definition(
            2,
            include_str!("fixtures/documentation/namespaces/types.luau"),
            1,
            "@game",
        )?,
        definition(
            3,
            include_str!("fixtures/documentation/namespaces/globals.luau"),
            1,
            "@game",
        )?,
    ];

    entry
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions
        .clone_from(&definitions);

    Ok((entry, definitions))
}

fn shared_declaration_fixture() -> io::Result<AnalysisRequest> {
    let mut input = request(vec![
        module(
            1,
            include_str!("fixtures/documentation/shared/input.luau"),
            1,
        )?,
        module(
            2,
            include_str!("fixtures/documentation/shared/input.luau"),
            1,
        )?,
    ])?;

    for (entry, namespace) in input.modules.iter_mut().zip(["@first", "@second"]) {
        entry
            .environment
            .as_mut()
            .expect("environment")
            .declarations[0]
            .definitions = vec![definition(
            3,
            include_str!("fixtures/documentation/declaration/definition.luau"),
            1,
            namespace,
        )?];
    }

    Ok(input)
}

fn shared_documentation(
    native: &mut Native,
    checked: &AnalysisResult,
    input: &AnalysisRequest,
) -> io::Result<()> {
    for (entry, namespace) in checked.modules.iter().zip(["@first", "@second"]) {
        assert!(entry.diagnostics.is_empty(), "{:?}", entry.diagnostics);

        let occurrence = native
            .inspect(
                &PositionInput {
                    analysis: entry.reference.clone().expect("updated reference"),
                    offset: input.modules[0]
                        .source
                        .source
                        .text
                        .find("= value")
                        .expect("value read")
                        + 2,
                },
                &options(Duration::from_secs(5)),
            )?
            .occurrence
            .expect("updated documentation");

        assert_eq!(
            occurrence.documentation,
            Knowledge::Known(Some(format!("{namespace}/global/value")))
        );
    }

    Ok(())
}

fn namespace_documentation(
    native: &mut Native,
    source: &str,
    reference: &AnalysisReference,
    symbols: Vec<[String; 2]>,
) -> io::Result<()> {
    for ([path, expected], expression) in
        symbols
            .into_iter()
            .zip(["library.call", "thing.Method", "thing,", "type"])
    {
        let offset = source.find(expression).expect("symbol expression")
            + expression.find('.').map_or(0, |separator| separator + 1);

        let occurrence = native
            .inspect(
                &PositionInput {
                    analysis: reference.clone(),
                    offset,
                },
                &options(Duration::from_secs(5)),
            )?
            .occurrence
            .expect("documentation occurrence");

        assert_eq!(
            occurrence.documentation,
            Knowledge::Known(Some(expected)),
            "{path}"
        );
    }

    Ok(())
}

#[test]
fn namespaces_preserve_builtins_members_and_file_diagnostics() -> io::Result<()> {
    let source = "return library.call, thing.Method, thing, type";
    let (mut entry, mut definitions) = namespace_fixture(source)?;

    let symbols: Vec<[String; 2]> = serde_json::from_str(include_str!(
        "fixtures/documentation/namespaces/expected.json"
    ))?;

    assert_eq!(symbols.len(), 4);
    let mut native = Native::default();

    let established = native.analyze(
        &request(vec![entry.clone()])?,
        &options(Duration::from_secs(5)),
    )?;

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("established reference");

    namespace_documentation(&mut native, source, &reference, symbols)?;

    for definition in &mut definitions {
        definition.namespace = "@renamed".to_owned();
    }

    entry.analysis.revision = 2;

    entry
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = definitions.clone();

    let established = native.analyze(
        &request(vec![entry.clone()])?,
        &options(Duration::from_secs(5)),
    )?;

    let occurrence = native
        .inspect(
            &PositionInput {
                analysis: established.modules[0]
                    .reference
                    .clone()
                    .expect("renamed reference"),
                offset: source.find("call").expect("member"),
            },
            &options(Duration::from_secs(5)),
        )?
        .occurrence
        .expect("member documentation");

    assert_eq!(
        occurrence.documentation,
        Knowledge::Known(Some("@renamed/global/library.call".to_owned()))
    );

    definitions[1] = definition(
        3,
        include_str!("fixtures/documentation/namespaces/invalid.luau"),
        2,
        "@renamed",
    )?;

    entry.analysis.revision = 3;

    entry
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = definitions.clone();

    let result = native.analyze(&request(vec![entry])?, &options(Duration::from_secs(5)))?;
    assert_ne!(result.completion, Completion::Complete);

    let diagnostics = &result.modules[0]
        .environment
        .as_ref()
        .expect("environment metadata")
        .diagnostics;

    assert_ne!(diagnostics.as_slice(), []);

    assert!(
        diagnostics
            .iter()
            .all(
                |diagnostic| diagnostic.primary.source == definitions[1].source.version
                    && diagnostic.primary.source.revision == 2
            )
    );

    native.finish()
}

#[test]
fn declaration_identity_is_shared_across_distinct_namespaces() -> io::Result<()> {
    let mut first = module(1, "return value", 1)?;
    let mut second = module(2, "return value", 1)?;

    for (entry, namespace) in [(&mut first, "@first"), (&mut second, "@second")] {
        entry
            .environment
            .as_mut()
            .expect("environment")
            .declarations[0]
            .definitions = vec![definition(
            3,
            include_str!("fixtures/documentation/declaration/definition.luau"),
            1,
            namespace,
        )?];
    }

    let mut native = Native::default();

    let result = native.analyze(
        &request(vec![first, second])?,
        &options(Duration::from_secs(5)),
    )?;

    for (entry, expected) in result
        .modules
        .iter()
        .zip(["@first/global/value", "@second/global/value"])
    {
        let occurrence = native
            .inspect(
                &PositionInput {
                    analysis: entry.reference.clone().expect("established reference"),
                    offset: 7,
                },
                &options(Duration::from_secs(5)),
            )?
            .occurrence
            .expect("global documentation");

        assert_eq!(
            occurrence.documentation,
            Knowledge::Known(Some(expected.to_owned()))
        );
    }

    native.finish()
}

#[test]
fn shared_declaration_revisions_update_atomically() -> io::Result<()> {
    let mut input = shared_declaration_fixture()?;

    let mut native = Native::default();
    native.analyze(&input, &options(Duration::from_secs(5)))?;

    let mut replacement = definition(
        3,
        include_str!("fixtures/documentation/shared/definition.luau"),
        2,
        "@first",
    )?;

    input.modules[0].analysis.revision = 2;

    input.modules[0]
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![replacement.clone()];

    assert!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))
            .is_err()
    );

    replacement.namespace = "@second".to_owned();
    input.modules[1].analysis.revision = 2;

    input.modules[1]
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![replacement.clone()];

    let checked = native.analyze(&input, &options(Duration::from_secs(5)))?;
    assert_eq!(checked.completion, Completion::Complete);

    shared_documentation(&mut native, &checked, &input)?;

    replacement.source = definition(
        3,
        include_str!("fixtures/documentation/declaration/definition.luau"),
        2,
        "@second",
    )?
    .source;

    input.modules[1]
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![replacement.clone()];

    assert!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))
            .is_err()
    );

    replacement.source.version.revision = 1;

    input.modules[1]
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![replacement];

    assert!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))
            .is_err()
    );

    input.modules[1]
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![definition(
        3,
        include_str!("fixtures/documentation/shared/definition.luau"),
        2,
        "@second",
    )?];

    assert_eq!(
        native
            .analyze(&input, &options(Duration::from_secs(5)))?
            .modules[1]
            .diagnostics
            .as_slice(),
        []
    );

    native.finish()
}

#[test]
fn documentation_preserves_interruption_and_environment_error_categories() -> io::Result<()> {
    let mut input = request(vec![module(1, "return print", 1)?])?;
    let mut native = Native::default();
    let cancelled = options(Duration::from_secs(5));
    cancelled.cancellation.cancel();

    assert_eq!(
        native.analyze(&input, &cancelled)?.completion,
        Completion::Cancelled
    );

    assert_eq!(
        native.analyze(&input, &options(Duration::ZERO))?.completion,
        Completion::TimedOut
    );

    input.modules[0]
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![definition(
        2,
        include_str!("fixtures/documentation/documentation/invalid.luau"),
        1,
        "@custom",
    )?];

    let unavailable = native.analyze(&input, &options(Duration::from_secs(5)))?;

    assert_eq!(
        unavailable.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert_eq!(
        unavailable.modules[0]
            .environment
            .as_ref()
            .expect("environment metadata")
            .availability,
        Availability::Unavailable(Reason::UnavailableEnvironment)
    );

    assert_ne!(
        unavailable.modules[0]
            .environment
            .as_ref()
            .expect("environment metadata")
            .diagnostics
            .as_slice(),
        []
    );

    native.finish()
}

#[test]
fn unrelated_type_errors_do_not_hide_builtin_documentation() -> io::Result<()> {
    let source = format!(
        "{}\nprint(value)",
        include_str!("fixtures/documentation/unrelated/input.luau").replace("return value", "")
    );

    let input = request(vec![module(1, &source, 1)?])?;
    let mut native = Native::default();
    let checked = native.analyze(&input, &options(Duration::from_secs(5)))?;
    assert_ne!(checked.modules[0].diagnostics.as_slice(), []);

    let occurrence = native
        .inspect(
            &PositionInput {
                analysis: checked.modules[0]
                    .reference
                    .clone()
                    .expect("partial established reference"),
                offset: source.find("print").expect("builtin"),
            },
            &options(Duration::from_secs(5)),
        )?
        .occurrence
        .expect("builtin documentation");

    assert_eq!(
        occurrence.documentation,
        Knowledge::Known(Some("@luau/global/print".to_owned()))
    );

    native.finish()
}
