//! Syntax and type diagnostics over prepared modules.

use std::{
    io,
    path::{Path, PathBuf},
};

use instar_bridge::{Checker, CheckerOptions};

use crate::{
    diagnostic::{self, Diagnostic},
    filter::Service,
    project::Project,
    session::{self, Host, Session},
};

/// Checks selected source files and their dependencies, reporting each diagnostic batch once.
///
/// # Errors
/// Returns source, configuration, preparation, or reporting failures.
pub fn run(
    project: &mut Project,
    paths: &[PathBuf],
    mut progress: impl FnMut(&Path),
    mut report: impl FnMut(&[Diagnostic]) -> io::Result<()>,
) -> io::Result<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    for environment in session::environments(project, paths, Some(Service::Check))? {
        let mut session = Session::new(project, &environment, &CheckerOptions::default())?;

        if session.declaration_diagnostics.is_empty() {
            session.prepare(project, &environment.entries, &mut |path| {
                progress(path);

                Ok(())
            })?;
        }

        let found = session.with_host(project, Some(Service::Check), None, |checker, host| {
            collect(checker, host)?;

            Ok(std::mem::take(&mut host.diagnostics))
        })?;

        report(&found)?;
        diagnostics.extend(found);
    }

    diagnostic::sort(&mut diagnostics);

    Ok(diagnostics)
}

pub(crate) fn collect(checker: &mut Checker, host: &mut Host<'_>) -> io::Result<()> {
    for diagnostic in host.declarations {
        if host.includes(&diagnostic.location.module.source)? {
            host.diagnostics.push(diagnostic.clone());
        }
    }

    for name in host.selected()? {
        checker.check(host, Path::new(&name))?;
    }

    host.report_timeouts()
}
