use serde_json::{Value, json};

pub(crate) fn diagnostic<'report>(
    report: &'report Value,
    code: &str,
    path: &str,
    range: [usize; 2],
) -> &'report Value {
    let findings = report["diagnostics"]
        .as_array()
        .expect("report diagnostics");
    let path = path.replace('\\', "/");
    let matches = findings
        .iter()
        .filter(|finding| {
            let actual = finding["path"]
                .as_str()
                .expect("diagnostic source path")
                .replace('\\', "/");
            finding["code"] == code
                && finding["range"] == json!(range)
                && (actual == path || actual.ends_with(&format!("/{path}")))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "expected one {code} diagnostic in {path}: {findings:#?}"
    );
    let finding = matches[0];
    assert!(finding["source"]["identity"].is_u64());
    assert!(finding["source"]["revision"].is_u64());
    assert_eq!(finding["revision"], finding["source"]["revision"]);
    if finding["identity"]["producer"] == "luau" && finding["category"] == "Lint" {
        let original = finding["identity"]["code"]
            .as_str()
            .expect("upstream warning code")
            .parse::<u32>()
            .expect("numeric upstream warning code");
        assert!(original > 0);
    } else {
        let identity_code = if code == "lint.directive" {
            format!(
                "{code}:{}",
                finding["message"].as_str().expect("finding message")
            )
        } else {
            code.to_owned()
        };
        assert_eq!(finding["identity"]["code"], identity_code);
    }
    assert_eq!(finding["identity"]["location"]["source"], finding["source"]);
    assert_eq!(
        finding["identity"]["location"]["span"],
        json!({"start":range[0],"end":range[1]})
    );
    assert!(
        finding["identity"]["inputs"]
            .as_array()
            .expect("finding input revisions")
            .contains(&finding["source"])
    );
    finding
}
