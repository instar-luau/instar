use std::{ops::Range, rc::Rc};

use vermis::{token::Symbol, tree::TokenIndex};

#[derive(Copy, Clone)]
pub(super) enum Layout {
    Fit,
    Prefix,
    Table,
    Vertical,
    Arguments,
}

#[derive(Clone)]
pub(super) enum Document {
    Token(TokenIndex),
    Verbatim(Range<usize>),
    Symbol(Symbol),
    Space,
    Line(usize),
    Soft(usize),

    Indent {
        conditional: bool,
        content: Vec<Self>,
    },

    Group {
        layout: Layout,
        content: Vec<Self>,
    },

    Choice {
        range: Range<usize>,
        prefix: bool,
        probe: Rc<[Self]>,
        inline: Rc<[Self]>,
        expanded: Rc<[Self]>,
    },
}
