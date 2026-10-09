use std::{collections::BTreeMap, time::Duration};

use instar_analysis::Location;
use serde::{Deserialize, Serialize};

use crate::{
    flags,
    frontend::{
        Declaration, Environment, Hierarchy, Hover, Link, LintResult, Reference, Signatures,
        Source, Suggestions,
    },
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
    References,

    Hover {
        line: u32,
        column: u32,
    },

    Definition {
        line: u32,
        column: u32,
        type_definition: bool,
    },

    Suggest {
        line: u32,
        column: u32,
    },

    Signatures {
        line: u32,
        column: u32,
    },

    Hierarchy {
        position: Option<[u32; 2]>,
    },
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Response {
    Parsed(Vec<Link>),
    Checked(instar_analysis::Result<String>),
    Linted(LintResult),
    Documentation(Option<String>),
    Hover(Option<Hover>),
    Locations(Vec<Location<String>>),
    Suggestions(Suggestions),
    Signatures(Option<Signatures>),
    References(Vec<Reference>),
    Hierarchy(Hierarchy),
}
