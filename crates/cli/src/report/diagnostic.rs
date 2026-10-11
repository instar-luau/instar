use std::{collections::BTreeMap, io, iter::once};

use codespan_reporting::{
    diagnostic::{Diagnostic as RenderedDiagnostic, Label},
    files::SimpleFiles,
    term::{
        self,
        termcolor::{Buffer, ColorChoice},
    },
};

use instar_project::{ProjectSnapshot, SourceOrigin};
use instar_shared::{Diagnostic, Location, Severity, SourceSnapshot, SourceVersion};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct Diagnostics {
    sources: BTreeMap<SourceVersion, (String, SourceSnapshot)>,
}

impl Diagnostics {
    pub(super) fn capture(&mut self, snapshot: &ProjectSnapshot) {
        for source in snapshot.sources() {
            if self.sources.contains_key(&source.version) {
                continue;
            }

            let name = match snapshot.origin(source.version.identity) {
                Some(SourceOrigin::Disk(path)) => path.display().to_string(),

                Some(SourceOrigin::Stdin { context }) => context.as_ref().map_or_else(
                    || "stdin.luau".to_owned(),
                    |path| path.display().to_string(),
                ),

                Some(
                    SourceOrigin::Document { address, .. } | SourceOrigin::Virtual { address },
                ) => address.clone(),

                None => format!("source:{}", source.version.identity.get()),
            };

            self.sources.insert(source.version, (name, source.clone()));
        }
    }

    pub(super) fn available(&self, diagnostic: &Diagnostic) -> bool {
        once(&diagnostic.primary)
            .chain(diagnostic.related.iter().map(|related| &related.location))
            .all(|location| {
                location.source.revision == 0 || self.sources.contains_key(&location.source)
            })
    }

    pub(super) fn render(
        &self,
        diagnostic: &Diagnostic,
        color: ColorChoice,
    ) -> io::Result<Vec<u8>> {
        let mut files = SimpleFiles::new();
        let mut labels = Vec::new();

        if let Some(label) = self.label(&mut files, &diagnostic.primary, true)? {
            labels.push(label);
        }

        for related in &diagnostic.related {
            if let Some(label) = self.label(&mut files, &related.location, false)? {
                labels.push(label.with_message(&related.message));
            }
        }

        let rendered = match diagnostic.severity {
            Severity::Error => RenderedDiagnostic::error(),
            Severity::Warning => RenderedDiagnostic::warning(),
            Severity::Information => RenderedDiagnostic::note(),
            Severity::Hint => RenderedDiagnostic::help(),
        }
        .with_code(&diagnostic.code)
        .with_message(&diagnostic.message)
        .with_labels(labels)
        .with_notes(vec![self.name(diagnostic.primary.source)]);

        let mut buffer = if matches!(color, ColorChoice::Always | ColorChoice::AlwaysAnsi) {
            Buffer::ansi()
        } else {
            Buffer::no_color()
        };

        term::emit_to_write_style(&mut buffer, &term::Config::default(), &files, &rendered)
            .map_err(io::Error::other)?;

        Ok(buffer.into_inner())
    }

    pub(super) fn record(&self, diagnostic: &Diagnostic) -> Value {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Information => "info",
            Severity::Hint => "hint",
        };

        json!({"identity":diagnostic.identity,"source":diagnostic.primary.source,"path":self.name(diagnostic.primary.source),"revision":diagnostic.primary.source.revision,"range":[diagnostic.primary.span.start,diagnostic.primary.span.end],"code":diagnostic.code,"category":diagnostic.category,"severity":severity,"message":diagnostic.message,"tags":diagnostic.tags,"related":diagnostic.related.iter().map(|related| json!({"source":related.location.source,"path":self.name(related.location.source),"revision":related.location.source.revision,"range":[related.location.span.start,related.location.span.end],"message":related.message})).collect::<Vec<_>>()})
    }

    pub(super) fn name(&self, version: SourceVersion) -> String {
        self.sources.get(&version).map_or_else(
            || format!("source:{}", version.identity.get()),
            |(name, _)| name.clone(),
        )
    }

    fn label(
        &self,
        files: &mut SimpleFiles<String, String>,
        location: &Location,
        primary: bool,
    ) -> io::Result<Option<Label<usize>>> {
        if location.source.revision == 0 {
            return Ok(None);
        }

        let (name, source) = self.sources.get(&location.source).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic source revision is unavailable",
            )
        })?;

        location.validate(source)?;
        let index = files.add(name.clone(), source.text.to_string());
        let range = location.span.start..location.span.end;

        Ok(Some(if primary {
            Label::primary(index, range)
        } else {
            Label::secondary(index, range)
        }))
    }
}
