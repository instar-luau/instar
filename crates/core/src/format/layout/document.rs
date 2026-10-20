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

#[derive(Copy, Clone)]
pub(super) enum Nesting {
    Always,
    Broken,
    Continuation,
}

#[derive(Clone)]
pub(super) struct Choice {
    pub(super) range: Range<usize>,
    pub(super) has_prefix: bool,
    pub(super) probe: Rc<[Document]>,
    pub(super) inline: Rc<[Document]>,
    pub(super) expanded: Rc<[Document]>,
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
        nesting: Nesting,
        content: Vec<Self>,
    },

    Group {
        layout: Layout,
        content: Vec<Self>,
    },

    Choice(Box<Choice>),
}
