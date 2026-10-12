use std::path::Path;

use instar_project::ProjectSnapshot;
use instar_shared::Diagnostic;
use serde_json::Value;

use super::fixture::Fixture;

pub(crate) fn attributed(findings: &[Diagnostic], captured: &ProjectSnapshot) -> Value {
    Value::Array(
        findings
            .iter()
            .map(|finding| {
                let mut value = serde_json::to_value(finding).expect("serialize diagnostic");

                value["primary"]["source"]["origin"] = serde_json::to_value(
                    captured
                        .origin(finding.primary.source.identity)
                        .expect("primary diagnostic origin"),
                )
                .expect("serialize primary origin");

                for (index, related) in finding.related.iter().enumerate() {
                    value["related"][index]["location"]["source"]["origin"] = serde_json::to_value(
                        captured
                            .origin(related.location.source.identity)
                            .expect("related diagnostic origin"),
                    )
                    .expect("serialize related origin");
                }

                value
            })
            .collect(),
    )
}

pub(crate) fn snapshot(fixture: &Fixture, group: &str, name: &str, value: &Value) {
    assert!(matches!(group, "check" | "lint" | "server"));
    let mut settings = insta::Settings::clone_current();

    settings.set_snapshot_path(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/contracts")
            .join(group),
    );

    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    settings.bind(|| insta::assert_json_snapshot!(name, fixture.normalize(value)));
}
