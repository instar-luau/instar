use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, FindingIdentity, Location, Options, Reason,
    Severity, SourceSnapshot, Span,
};

use vermis::tree::{NodeKind, Tree as SyntaxTree};

use crate::SyntaxSettings;

/// Request-local lossless syntax and its exact immutable inputs.
#[derive(Debug)]
pub struct Tree<'source> {
    snapshot: &'source SourceSnapshot,
    settings: SyntaxSettings,
    syntax: SyntaxTree<'source>,
    coverage: Completion,
}

impl<'source> Tree<'source> {
    /// Returns the exact source snapshot.
    #[must_use]
    pub fn snapshot(&self) -> &'source SourceSnapshot {
        self.snapshot
    }

    /// Returns the selected grammar context.
    #[must_use]
    pub fn settings(&self) -> &SyntaxSettings {
        &self.settings
    }

    /// Returns the indexed lossless syntax with original byte coordinates.
    #[must_use]
    pub fn syntax(&self) -> &SyntaxTree<'source> {
        &self.syntax
    }

    /// Returns parser profile coverage.
    #[must_use]
    pub fn coverage(&self) -> &Completion {
        &self.coverage
    }
}

/// Parsed syntax, diagnostics and unavailable recovery regions.
#[derive(Debug)]
pub struct SyntaxResult<'source> {
    /// Request-local syntax.
    pub tree: Tree<'source>,

    /// Revision-attributed parser findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Malformed syntax spans.
    pub recovery: Vec<Span>,

    /// Whether the selected parser profile completed.
    pub completion: Completion,
}

/// Parses one snapshot under the selected grammar profile.
#[must_use]
pub fn parse<'source>(
    source: &'source SourceSnapshot,
    settings: &SyntaxSettings,
    options: &Options,
) -> SyntaxResult<'source> {
    let prefix = usize::from(source.text.starts_with('\u{feff}')) * 3;

    let bytes = if options.interrupted().is_some() {
        &source.text.as_bytes()[prefix..prefix]
    } else {
        &source.text.as_bytes()[prefix..]
    };

    let mut syntax = vermis::parse(bytes);
    syntax.source = source.text.as_bytes();

    for token in &mut syntax.tokens {
        token.span.start += prefix;
        token.span.end += prefix;
    }

    for node in &mut syntax.nodes {
        node.span.start += prefix;
        node.span.end += prefix;
    }

    for diagnostic in &mut syntax.diagnostics {
        diagnostic.span.start += prefix;
        diagnostic.span.end += prefix;
    }

    let mut completion = options.interrupted().unwrap_or(Completion::Complete);

    let mut diagnostics = syntax
        .diagnostics
        .iter()
        .map(|finding| {
            diagnostic(
                source,
                Span {
                    start: finding.span.start,
                    end: finding.span.end,
                },
                "syntax.parse",
                finding.message,
                DiagnosticCategory::Syntax,
            )
        })
        .collect::<Vec<_>>();

    if source.validate().is_err() || prefix > 0 && !settings.encoding_marker {
        diagnostics.push(diagnostic(
            source,
            Span {
                start: 0,
                end: prefix,
            },
            "syntax.encoding",
            "unsupported source encoding context",
            DiagnosticCategory::Syntax,
        ));

        completion = completion.combine(Completion::Incomplete(vec![Reason::UnsupportedFeature]));
    }

    if settings
        .features
        .iter()
        .any(|feature| feature != "LuauExperimentalIfLocalSyntax=true")
    {
        diagnostics.push(diagnostic(
            source,
            Span {
                start: prefix,
                end: prefix,
            },
            "syntax.profile",
            "Vermis does not expose the requested grammar feature profile",
            DiagnosticCategory::Syntax,
        ));

        completion = completion.combine(Completion::Incomplete(vec![Reason::UnsupportedFeature]));
    }

    if !settings.declaration_syntax && let Some(node)=syntax.nodes.iter().find(|node| {
        matches!(node.kind,NodeKind::Declaration {..})
            || matches!(&node.kind,NodeKind::Function {prefix:Some(prefix),..} if syntax.token(*prefix).bytes(syntax.source)==b"declare")
    }) {
        diagnostics.push(diagnostic(source, Span {start:node.span.start,end:node.span.end}, "syntax.declaration", "declaration syntax is disabled", DiagnosticCategory::Syntax));
        completion = completion.combine(Completion::Incomplete(vec![Reason::UnsupportedSyntax]));
    }

    let mut recovery = syntax
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Error | NodeKind::Missing { .. }))
        .map(|node| Span {
            start: node.span.start,
            end: node.span.end,
        })
        .collect::<Vec<_>>();

    recovery.extend(syntax.diagnostics.iter().map(|finding| Span {
        start: finding.span.start,
        end: finding.span.end,
    }));

    recovery.sort_by_key(|span| (span.start, span.end));
    recovery.dedup();

    SyntaxResult {
        tree: Tree {
            snapshot: source,
            settings: settings.clone(),
            syntax,
            coverage: completion.clone(),
        },
        diagnostics,
        recovery,
        completion,
    }
}

pub(crate) fn diagnostic(
    source: &SourceSnapshot,
    span: Span,
    code: &str,
    message: &str,
    category: DiagnosticCategory,
) -> Diagnostic {
    let primary = Location {
        source: source.version,
        span,
    };

    Diagnostic {
        identity: FindingIdentity::new("syntax", vec![source.version], code, primary),
        code: code.to_owned(),
        category,
        severity: Severity::Error,
        message: message.to_owned(),
        primary,
        related: Vec::new(),
        tags: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use instar_shared::{
        Completion, Options, Reason, SourceIdentity, SourceSnapshot, SourceVersion,
    };

    use crate::{SyntaxSettings, imports, parse, tree::NodeKind};

    #[test]
    fn conditional_local_syntax_accepts_only_the_supported_positive_profile() {
        let source = SourceSnapshot::from_bytes(
            SourceVersion {
                identity: SourceIdentity::new(1),
                revision: 2,
            },
            b"local path='./target'\nlocal result=if local path=path then require(path) else require(path)\nrequire(path)",
        )
        .expect("UTF-8 conditional local source");

        let options = Options::default();

        for features in [
            Vec::new(),
            vec!["LuauExperimentalIfLocalSyntax=true".to_owned()],
        ] {
            let settings = SyntaxSettings {
                features,
                ..SyntaxSettings::default()
            };

            let parsed = parse(&source, &settings, &options);
            assert_eq!(parsed.completion, Completion::Complete);
            assert_eq!(parsed.tree.coverage(), &Completion::Complete);
            assert_eq!(parsed.tree.settings(), &settings);
            assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
            assert!(parsed.recovery.is_empty());
            let tree = parsed.tree.syntax();

            assert!(tree.nodes.iter().any(|node| matches!(
                &node.kind,
                NodeKind::Conditional { condition, .. }
                    if matches!(tree.node(*condition).kind, NodeKind::Local { .. })
            )));

            let imports = imports(&parsed.tree, &options);
            assert_eq!(imports.len(), 3);

            for import in imports {
                assert_eq!(import.source, source.version);
                assert_eq!(import.request, None);

                assert_eq!(
                    &source.text[import.expression.start..import.expression.end],
                    "path"
                );
            }
        }

        for features in [
            vec!["LuauExperimentalIfLocalSyntax=false"],
            vec!["LuauExperimentalIfLocalAnalysis=true"],
            vec!["unknown=true"],
            vec!["LuauExperimentalIfLocalSyntax=true", "unknown=true"],
        ] {
            let settings = SyntaxSettings {
                features: features.into_iter().map(str::to_owned).collect(),
                ..SyntaxSettings::default()
            };

            let parsed = parse(&source, &settings, &options);

            assert_eq!(
                parsed.completion,
                Completion::Incomplete(vec![Reason::UnsupportedFeature])
            );

            assert_eq!(parsed.tree.coverage(), &parsed.completion);
            assert_eq!(parsed.diagnostics.len(), 1);
            assert_eq!(parsed.diagnostics[0].code, "syntax.profile");
            assert_eq!(parsed.diagnostics[0].primary.source, source.version);
        }
    }
}
