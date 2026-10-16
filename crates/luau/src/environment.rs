/// Ordered declarations and class capabilities for one named environment.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    /// Declaration files in application order.
    pub definitions: Vec<Definition>,

    /// Optional class permissions; an empty list grants no construction or service capabilities.
    pub classes: Option<Vec<Class>>,
}

/// A declaration source with its diagnostic identity and documentation namespace.
#[derive(Clone, Debug)]
pub struct Definition {
    /// Host-owned source identity used in diagnostics.
    pub name: String,

    /// Documentation symbol namespace.
    pub namespace: String,

    /// Complete declaration source text.
    pub source: String,
}

/// Capabilities applied to a declared Roblox class.
#[derive(Clone, Debug)]
pub struct Class {
    /// Declared class name.
    pub name: String,

    /// Whether the class is available through `GetService`.
    pub service: bool,

    /// Whether the class is available through `Instance.new`.
    pub creatable: bool,

    /// Independent property permissions.
    pub properties: Vec<Property>,
}

/// Read and write permissions applied to a declared property.
#[derive(Clone, Debug)]
pub struct Property {
    /// Declared property name.
    pub name: String,

    /// Whether reads are permitted.
    pub read: bool,

    /// Whether writes are permitted.
    pub write: bool,
}
