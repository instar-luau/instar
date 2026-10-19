use std::{cell::OnceCell, iter};

use vermis::tree::{NodeIndex, Tree};

use crate::{
    lint::{Edit, Finding, Prepared, Symbols},
    syntax::bindings::Bindings,
};

pub(super) struct Context<'tree, 'source> {
    pub(super) tree: &'tree Tree<'source>,
    pub(super) source: &'source str,
    pub(super) settings: &'tree Prepared,
    pub(super) symbols: &'tree Symbols,
    pub(super) bindings: &'tree OnceCell<Bindings>,
    pub(super) findings: Vec<Finding>,
    pub(super) lines: &'tree OnceCell<Vec<usize>>,
}

impl<'tree> Context<'tree, '_> {
    pub(super) fn active(&self, rules: &[&str]) -> bool {
        rules
            .iter()
            .any(|rule| self.settings.enabled(rule).is_some())
    }

    pub(super) fn bindings(&self) -> &'tree Bindings {
        self.bindings.get_or_init(|| Bindings::new(self.tree))
    }

    pub(super) fn lines(&self) -> &'tree [usize] {
        self.lines.get_or_init(|| {
            iter::once(0)
                .chain(
                    self.source
                        .match_indices('\n')
                        .map(|(offset, _)| offset + 1),
                )
                .collect()
        })
    }

    pub(super) fn emit(&mut self, node: NodeIndex, rule: &str, message: &str, edits: Vec<Edit>) {
        if let Some(severity) = self.settings.enabled(rule) {
            let span = self.tree.node(node).span;

            self.findings.push(Finding {
                rule: rule.to_owned(),
                severity,
                range: span.start..span.end,
                message: message.to_owned(),
                edits,
            });
        }
    }

    pub(super) fn warn(&mut self, node: NodeIndex, rule: &str, message: &str) {
        self.emit(node, rule, message, Vec::new());
    }
}
