use vermis::{token::Symbol, tree::TokenIndex};

#[derive(Clone, Copy)]
pub(super) enum Layout {
    Fit,
    Vertical,
    Arguments,
}

pub(super) enum Document {
    Token(TokenIndex),
    Symbol(Symbol),
    Space,
    Line(usize),
    Soft(usize),
    TrailingComma,

    Indent {
        conditional: bool,
        content: Vec<Self>,
    },

    Group {
        layout: Layout,
        content: Vec<Self>,
    },
}

impl Document {
    pub(super) fn multiline(&self, tree: &vermis::tree::Tree<'_>) -> bool {
        match self {
            Self::Line(_)
            | Self::Group {
                layout: Layout::Vertical,
                ..
            } => true,

            Self::Token(index) => tree.token(*index).bytes(tree.source).contains(&b'\n'),

            Self::Group { content, .. } | Self::Indent { content, .. } => {
                content.iter().any(|item| item.multiline(tree))
            }

            _ => false,
        }
    }
}
