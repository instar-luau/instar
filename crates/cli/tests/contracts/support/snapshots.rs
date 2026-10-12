use std::path::Path;

use serde_json::Value;

use super::fixture::Fixture;

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
