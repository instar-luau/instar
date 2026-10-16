/// A registered native Luau flag override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// A boolean flag value.
    Boolean(bool),

    /// A signed 32-bit integer flag value.
    Integer(i32),
}
