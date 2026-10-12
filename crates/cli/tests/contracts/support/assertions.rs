use instar_project::{ProjectSnapshot, SourceOrigin};
use instar_shared::{Diagnostic, DiagnosticCategory};

pub(crate) fn diagnostic<'findings>(
    findings: &'findings [Diagnostic],
    snapshot: &ProjectSnapshot,
    code: &str,
    path: &str,
    range: [usize; 2],
) -> &'findings Diagnostic {
    let path = path.replace('\\', "/");

    let matches = findings
        .iter()
        .filter(|finding| {
            let actual = match snapshot.origin(finding.primary.source.identity) {
                Some(SourceOrigin::Disk(path)) => path.display().to_string(),

                Some(SourceOrigin::Stdin { context }) => context.as_ref().map_or_else(
                    || "stdin.luau".to_owned(),
                    |path| path.display().to_string(),
                ),

                Some(
                    SourceOrigin::Document { address, .. } | SourceOrigin::Virtual { address },
                ) => address.clone(),

                None => format!("source:{}", finding.primary.source.identity.get()),
            };

            let actual = actual.replace('\\', "/");

            finding.code == code
                && [finding.primary.span.start, finding.primary.span.end] == range
                && (actual == path || actual.ends_with(&format!("/{path}")))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        matches.len(),
        1,
        "expected one {code} diagnostic in {path}: {findings:#?}"
    );

    let finding = matches[0];

    let source = snapshot
        .source(finding.primary.source.identity)
        .expect("diagnostic source snapshot");

    assert_eq!(finding.primary.source, source.version);

    if finding.identity.producer() == "luau" && finding.category == DiagnosticCategory::Lint {
        let original = finding
            .identity
            .code()
            .parse::<u32>()
            .expect("numeric upstream warning code");

        assert!(original > 0);
    } else {
        let identity_code = if code == "lint.directive" {
            format!("{code}:{}", finding.message)
        } else {
            code.to_owned()
        };

        assert_eq!(finding.identity.code(), identity_code);
    }

    assert_eq!(finding.identity.location(), finding.primary);
    assert!(finding.identity.inputs().contains(&finding.primary.source));

    finding
}
