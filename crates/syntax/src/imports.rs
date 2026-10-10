use instar_shared::{Options, SourceVersion, Span};
use serde::{Deserialize, Serialize};
use vermis::tree::{NodeIndex, NodeKind, Tree as SyntaxTree};

use crate::{BindingResult, Tree, bindings, literal};

/// A lexical global require call and its argument syntax.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SyntaxImport {
    /// Exact revision supplying this require syntax.
    pub source: SourceVersion,

    /// Complete require call.
    pub call: Span,

    /// Single require argument.
    pub argument: Span,

    /// Decoded literal module request, when present.
    pub request: Option<String>,

    /// Argument expression span.
    pub expression: Span,
}

/// Collects recognized global require calls without resolution or inference.
#[must_use]
pub fn imports(tree: &Tree<'_>, options: &Options) -> Vec<SyntaxImport> {
    let bindings = bindings(tree, options);
    let syntax = tree.syntax();
    let mut imports = Vec::new();

    for (index, node) in syntax.nodes.iter().enumerate() {
        if options.interrupted().is_some() {
            break;
        }

        if let Some(argument) = require(syntax, NodeIndex::new(index), &bindings) {
            let argument_span = syntax.node(argument).span;

            imports.push(SyntaxImport {
                source: tree.snapshot().version,
                call: Span {
                    start: node.span.start,
                    end: node.span.end,
                },
                argument: Span {
                    start: argument_span.start,
                    end: argument_span.end,
                },
                request: literal::string(syntax, argument),
                expression: Span {
                    start: argument_span.start,
                    end: argument_span.end,
                },
            });
        }
    }

    imports.sort_by_key(|import| import.call.start);

    imports
}

pub(crate) fn require(
    syntax: &SyntaxTree<'_>,
    node: NodeIndex,
    bindings: &BindingResult,
) -> Option<NodeIndex> {
    let NodeKind::Call { callee, arguments } = &syntax.node(node).kind else {
        return None;
    };

    if !matches!(syntax.node(*callee).kind, NodeKind::Name { .. })
        || syntax.text(*callee) != b"require"
        || !bindings.global(syntax, *callee)
    {
        return None;
    }

    let NodeKind::Arguments { values, .. } = &syntax.node(*arguments).kind else {
        return None;
    };

    let [argument] = syntax.list(values) else {
        return None;
    };

    Some(argument.node)
}
