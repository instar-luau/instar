macro_rules! identities {
    ($($field:ident => $identity:ident: $settings:ident, $documentation:literal;)*) => {
        /// An Instar lint rule identity.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Rule {
            $(#[doc = $documentation] $identity,)*
        }

        impl Rule {
            /// Returns the configuration and presentation name.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self { $(Self::$identity => stringify!($field),)* }
            }
        }
    };
}

crate::inventory::inventory!(identities);

impl std::fmt::Display for Rule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}
