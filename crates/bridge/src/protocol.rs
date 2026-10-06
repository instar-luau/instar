use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};

use crate::frontend::{Link, LintResult, Source};

#[derive(Deserialize, Serialize)]
pub(crate) struct Request {
    pub(crate) sources: BTreeMap<String, Source>,
    pub(crate) entries: Vec<String>,
    pub(crate) modules: Vec<String>,
    pub(crate) operation: Operation,
    pub(crate) timeout: Duration,
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Operation {
    Parse,
    Check,
    Lint(Vec<String>),
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Response {
    Parsed(Vec<Link>),
    Checked(instar_analysis::Result<String>),
    Linted(LintResult),
}
