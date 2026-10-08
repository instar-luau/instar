use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    flags,
    frontend::{Declaration, Environment, Link, LintResult, Source},
};

#[derive(Deserialize, Serialize)]
pub(crate) struct Request {
    pub(crate) sources: BTreeMap<String, Source>,
    pub(crate) declarations: BTreeMap<String, Declaration>,
    pub(crate) environments: BTreeMap<u64, Environment>,
    pub(crate) entries: Vec<String>,
    pub(crate) modules: Vec<String>,
    pub(crate) operation: Operation,
    pub(crate) timeout: Duration,
    pub(crate) flags: BTreeMap<String, flags::Value>,
    pub(crate) progress: bool,
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Operation {
    Parse,
    Check,
    Documentation(String),
    Lint(Vec<String>),
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Response {
    Parsed(Vec<Link>),
    Checked(instar_analysis::Result<String>),
    Linted(LintResult),
    Documentation(Option<String>),
}
