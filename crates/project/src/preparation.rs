use std::{io, sync::Arc};

use instar_luau::ImportSite;
use instar_shared::{Completion, Diagnostic, Options, SourceSnapshot};
use instar_syntax::SyntaxSettings;

use crate::{
    capture::Capture,
    configuration::{EffectiveSettings, worker_options},
    execution::{Pending, Runtime, Work, WorkOutput},
    extract::extract,
    graph::syntax_settings,
    selection::SelectedSource,
};

pub(crate) struct SyntaxInput {
    pub(crate) source: SourceSnapshot,
    pub(crate) settings: SyntaxSettings,
}

pub(crate) struct SyntaxResult {
    pub(crate) completion: Completion,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) imports: io::Result<Vec<ImportSite>>,
}

pub(crate) enum Preparation {
    Pending(Pending),
    Ready(Arc<SyntaxResult>),
}

pub(crate) struct SyntaxEntry {
    pub(crate) settings: SyntaxSettings,
    pub(crate) preparation: Preparation,
}

impl Runtime {
    pub(crate) fn prepare_target(
        &mut self,
        target: &SelectedSource,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<()> {
        let source = capture
            .snapshot
            .source(target.identity)
            .ok_or_else(|| io::Error::other("selected preparation source is absent"))?;

        self.prepare_source(source, &target.effective, options)
    }

    pub(crate) fn prepare_source(
        &mut self,
        source: &SourceSnapshot,
        effective: &EffectiveSettings,
        options: &Options,
    ) -> io::Result<()> {
        options.check()?;
        let settings = syntax_settings(effective);

        if self
            .cache
            .syntax
            .get(&source.version)
            .is_some_and(|entry| entry.settings == settings)
        {
            return Ok(());
        }

        let pending = self.workers.submit(
            Work::Syntax(SyntaxInput {
                source: source.clone(),
                settings: settings.clone(),
            }),
            &worker_options(options),
        )?;

        self.cache.syntax.insert(
            source.version,
            SyntaxEntry {
                settings,
                preparation: Preparation::Pending(pending),
            },
        );

        Ok(())
    }

    pub(crate) fn source_syntax(
        &mut self,
        source: &SourceSnapshot,
        effective: &EffectiveSettings,
        options: &Options,
    ) -> io::Result<Arc<SyntaxResult>> {
        self.prepare_source(source, effective, options)?;

        let entry = self
            .cache
            .syntax
            .remove(&source.version)
            .ok_or_else(|| io::Error::other("source preparation is absent"))?;

        let result = match entry.preparation {
            Preparation::Ready(result) => result,

            Preparation::Pending(pending) => match self.workers.receive(pending, options)? {
                WorkOutput::Syntax(result) => Arc::new(result),
                _ => return Err(io::Error::other("syntax lane returned wrong outcome")),
            },
        };

        if result.completion == Completion::Complete && result.imports.is_ok() {
            self.cache.syntax.insert(
                source.version,
                SyntaxEntry {
                    settings: entry.settings,
                    preparation: Preparation::Ready(Arc::clone(&result)),
                },
            );
        }

        Ok(result)
    }

    pub(crate) fn finish_preparation(&mut self) {
        let snapshot = self.host.snapshot();

        self.cache.syntax.retain(|version, entry| {
            matches!(entry.preparation, Preparation::Ready(_))
                && snapshot
                    .source(version.identity)
                    .is_some_and(|source| source.version == *version)
        });
    }
}

pub(crate) fn parse(input: &SyntaxInput, options: &Options) -> SyntaxResult {
    let syntax = instar_syntax::parse(&input.source, &input.settings, options);
    let imports = extract(&syntax.tree, options);

    SyntaxResult {
        completion: syntax.completion,
        diagnostics: syntax.diagnostics,
        imports,
    }
}
