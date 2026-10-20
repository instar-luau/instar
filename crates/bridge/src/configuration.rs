use std::collections::BTreeMap;

use crate::{flags::Flag, instances::Location};

/// Resolved type-checking configuration supplied by the host.
#[derive(Clone, Debug, Default)]
pub struct Configuration {
    /// Default mode, subject to the module's mode directive.
    pub mode: Mode,

    /// Additional globals accepted by this module.
    pub globals: Vec<String>,

    /// Named declaration environment, or builtin globals alone.
    pub environment: Option<String>,

    /// Native flags required by this module.
    pub flags: BTreeMap<String, Flag>,

    /// Instance locations in which this module is checked.
    pub locations: Vec<Location>,
}

/// Luau's default checking mode for a module.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    /// Parse without checking types.
    NoCheck,

    /// Infer types with permissive unannotated values.
    #[default]
    NonStrict,

    /// Check inferred and annotated types strictly.
    Strict,
}
