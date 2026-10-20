mod colors;
mod folding;
mod hints;
mod selection;
mod tokens;

pub(crate) use self::{
    colors::{color_presentation, document_color},
    folding::folding_range,
    hints::{inlay_hint, inlay_hint_resolve, inlay_hint_resolve_source, inlay_hint_source},
    selection::selection_range,
    tokens::{legend, semantic_tokens_full, semantic_tokens_full_delta, semantic_tokens_range},
};
