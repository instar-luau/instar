/// A registered native Luau flag override.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Flag {
    /// A boolean flag value.
    Boolean(bool),

    /// A signed 32-bit integer flag value.
    Integer(i32),
}
