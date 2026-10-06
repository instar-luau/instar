mod assignments;
mod bindings;
mod calls;
mod control;
mod expressions;
mod roblox;
mod syntax;

use std::io;
use std::time::Instant;

use vermis::token::Span;
use vermis::tree::{NodeIndex, NodeKind, Tree};

use crate::{Completion, Diagnostic, Kind, Level, Location, Options, Reason, Result, Rule, Source};
use instar_syntax::bindings::Bindings;

struct Context<'tree, 'source> {
    tree: &'tree Tree<'source>,
    source: &'tree Source<'source>,
    bindings: Bindings,
    ignored: regex::Regex,
    findings: Vec<(Span, Rule, String)>,
}

/// Evaluates syntax and lexical binding rules without native analysis or fixes.
///
/// # Errors
/// Returns invalid rule configuration; parse failures are structured diagnostics.
pub fn lint<Module: Clone>(
    module: Module,
    source: &Source<'_>,
    options: &Options,
) -> io::Result<Result<Module>> {
    let started = Instant::now();
    let ignored = source.configuration.validate()?;

    let mut result = Result {
        modules: vec![module.clone()],
        diagnostics: Vec::new(),
        completion: Completion::Complete,
    };

    if let Some(reason) = options.interrupted(started) {
        result.completion = Completion::Incomplete(reason);

        return Ok(result);
    }

    validate_ranges(source)?;
    let tree = vermis::parse(source.text.as_bytes());

    if let Some(reason) = options.interrupted(started) {
        result.completion = Completion::Incomplete(reason);

        return Ok(result);
    }

    if !tree.diagnostics.is_empty() {
        for diagnostic in &tree.diagnostics {
            result.diagnostics.push(Diagnostic {
                location: Location {
                    module: module.clone(),
                    revision: source.revision,
                    range: [diagnostic.span.start, diagnostic.span.end],
                },
                kind: Kind::Analysis(instar_analysis::Kind::Syntax { code: None }),
                level: Level::Deny,
                message: diagnostic.message.to_owned(),
                related: Vec::new(),
            });
        }

        return Ok(result);
    }

    let bindings = Bindings::analyze(&tree, options, started);

    if let Some(reason) = bindings.interruption {
        result.completion = Completion::Incomplete(reason);

        return Ok(result);
    }

    let mut context = Context {
        tree: &tree,
        source,
        bindings,
        ignored,
        findings: Vec::new(),
    };

    context.declarations();
    let mut pending = vec![(tree.root, Vec::new())];

    while let Some((node, ancestors)) = pending.pop() {
        if let Some(reason) = options.interrupted(started) {
            result.completion = Completion::Incomplete(reason);
            break;
        }

        assignments::check(&mut context, node, &ancestors);
        bindings::check(&mut context, node, &ancestors);
        calls::check(&mut context, node, &ancestors);
        control::check(&mut context, node, &ancestors);
        expressions::check(&mut context, node, &ancestors);
        roblox::check(&mut context, node);
        let mut ancestors = ancestors;
        ancestors.push(node);

        for child in tree.children(node).into_iter().rev() {
            pending.push((child, ancestors.clone()));
        }
    }

    context.requires();
    context.inferences(&module, &mut result)?;

    context
        .findings
        .sort_by_key(|(span, rule, _)| (span.start, span.end, *rule));

    context.findings.dedup();

    for (span, rule, message) in context.findings {
        result.diagnostics.push(Diagnostic {
            location: Location {
                module: module.clone(),
                revision: source.revision,
                range: [span.start, span.end],
            },
            kind: Kind::Rule(rule),
            level: source.configuration.level(rule),
            message,
            related: Vec::new(),
        });
    }

    Ok(result)
}

fn valid_range(text: &str, range: [usize; 2]) -> bool {
    range[0] <= range[1]
        && range[1] <= text.len()
        && text.is_char_boundary(range[0])
        && text.is_char_boundary(range[1])
}

fn validate_ranges(source: &Source<'_>) -> io::Result<()> {
    for site in source.requires {
        if !valid_range(source.text, site.call)
            || !valid_range(source.text, site.argument)
            || site.call[0] > site.argument[0]
            || site.argument[1] > site.call[1]
            || site.path.is_some() && !site.constant
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "require site is not anchored to source bytes",
            ));
        }
    }

    Ok(())
}

impl Context<'_, '_> {
    fn inferences<Module: Clone>(
        &mut self,
        module: &Module,
        result: &mut Result<Module>,
    ) -> io::Result<()> {
        let source = self.source;

        if let Some(inferred) = source.inferred {
            for inference in inferred {
                if !valid_range(source.text, inference.range) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "native inference is not anchored to source bytes",
                    ));
                }

                let rule = if inference.parameter {
                    Rule::ImplicitAnyParameter
                } else {
                    Rule::ImplicitAnyLocal
                };

                self.emit(
                    Span {
                        start: inference.range[0],
                        end: inference.range[1],
                    },
                    rule,
                    "unannotated binding has inferred any type",
                );
            }
        } else if source.configuration.semantic() {
            for rule in [Rule::ImplicitAnyLocal, Rule::ImplicitAnyParameter] {
                if source.configuration.level(rule) != Level::Allow {
                    result.diagnostics.push(Diagnostic {
                        location: Location {
                            module: module.clone(),
                            revision: source.revision,
                            range: [0, 0],
                        },
                        kind: Kind::Analysis(instar_analysis::Kind::Unsupported),
                        level: Level::Deny,
                        message: format!("{rule} requires shared native inferred binding types"),
                        related: Vec::new(),
                    });
                }
            }

            if result.completion == Completion::Complete {
                result.completion = Completion::Incomplete(Reason::Unsupported);
            }
        }

        Ok(())
    }

    fn emit(&mut self, span: Span, rule: Rule, message: impl Into<String>) {
        if self.source.configuration.level(rule) != Level::Allow {
            self.findings.push((span, rule, message.into()));
        }
    }

    fn finding(&mut self, node: NodeIndex, rule: Rule, message: impl Into<String>) {
        self.emit(self.tree.node(node).span, rule, message);
    }

    fn global(&self, node: NodeIndex, name: &[u8]) -> bool {
        matches!(self.tree.node(node).kind, NodeKind::Name { .. })
            && self.tree.text(node) == name
            && self.bindings.global(self.tree, node)
    }
}
