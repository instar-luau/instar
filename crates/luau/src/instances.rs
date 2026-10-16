/// One instance in a hierarchy ordered with parents before their children.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Runtime instance name.
    pub name: String,

    /// Declared Roblox class name.
    pub class: String,

    /// Parent index, absent only for the hierarchy root.
    pub parent: Option<usize>,

    /// Canonical source identity, present when the instance has one unambiguous source.
    pub module: Option<String>,
}

/// A module's location within an instance hierarchy.
#[derive(Clone, Debug)]
pub struct Location {
    /// Named instance hierarchy.
    pub hierarchy: String,

    /// Script index, absent for sources outside the hierarchy.
    pub script: Option<usize>,
}
