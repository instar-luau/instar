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
pub(super) struct Choice {
    pub(super) range: Range<usize>,
    pub(super) prefix: bool,
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
        conditional: bool,
        content: Vec<Self>,
    },

    Group {
        layout: Layout,
        content: Vec<Self>,
    },

    Choice(Box<Choice>),
}
