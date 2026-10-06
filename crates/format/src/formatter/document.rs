use vermis::{token::Symbol, tree::TokenIndex};

#[derive(Clone, Copy)]
pub(super) enum Layout {
    Fit,
    Vertical,
    Pressed,
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

pub(super) fn width(
    document: &[Document],
    tree: &vermis::tree::Tree<'_>,
    quotes: &[Option<Vec<u8>>],
    options: &instar_analysis::Options,
    started: std::time::Instant,
) -> Option<usize> {
    document.iter().try_fold(0usize, |width, item| {
        if options.interrupted(started).is_some() {
            return None;
        }

        let length = match item {
            Document::Token(index) => {
                let bytes = quotes[index.get()]
                    .as_deref()
                    .unwrap_or_else(|| tree.token(*index).bytes(tree.source));

                if bytes.contains(&b'\n') {
                    return None;
                }

                bytes.len()
            }

            Document::Symbol(_) | Document::Space => 1,
            Document::Soft(spaces) => *spaces,
            Document::TrailingComma => 0,

            Document::Line(_)
            | Document::Group {
                layout: Layout::Vertical,
                ..
            } => return None,

            Document::Group { content, .. } | Document::Indent { content, .. } => {
                self::width(content, tree, quotes, options, started)?
            }
        };

        width.checked_add(length)
    })
}
