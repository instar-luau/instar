//! Incremental editor preparation and diagnostic composition.

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

use instar_bridge::{Callbacks, Checker, CheckerOptions};

use crate::{
    check,
    diagnostic::{self, Diagnostic},
    filter::Service,
    invalid, lint,
    project::Project,
    session::{self, Session, is_declaration_source},
};

/// Persistent editor state. Keep this value on its owning worker thread.
#[derive(Default)]
pub struct Editor {
    project: Project,
    open: BTreeSet<PathBuf>,
    sessions: Vec<Session>,
}

impl Editor {
    /// Updates or closes an unsaved document and invalidates its dependents.
    ///
    /// # Errors
    /// Returns invalid-path or native invalidation errors.
    pub fn set_source(&mut self, path: &Path, text: Option<&str>) -> io::Result<()> {
        let path = crate::absolute(path)?;

        let topology_changed =
            self.project.overlay_file(&path) != text.is_some() && !path.is_file();

        self.project.set_source(&path, text)?;

        if is_declaration_source(&path)
            || self.sessions.iter().any(|session| {
                session
                    .definitions
                    .iter()
                    .any(|(_, location)| Path::new(location) == path)
            })
        {
            self.sessions.clear();
        }

        if text.is_some() {
            self.open.insert(path.clone());
        } else {
            self.open.remove(&path);
        }

        for session in &mut self.sessions {
            session.invalidate(&path, topology_changed)?;
        }

        Ok(())
    }

    /// Reloads disk/configuration state while preserving unsaved buffers.
    pub fn refresh(&mut self) {
        self.sessions.clear();
        self.project.refresh();
    }

    /// Prepares semantic state for open documents without running diagnostic passes.
    ///
    /// # Errors
    /// Returns source, configuration, or native preparation failures.
    pub fn prepare(&mut self) -> io::Result<()> {
        self.prepare_roots(&[], &mut |_, _| true)
    }

    /// Prepares additional workspace roots for navigation.
    ///
    /// # Errors
    /// Returns source, configuration, or native preparation failures.
    pub fn index(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        self.prepare_roots(paths, &mut |_, _| true)
    }

    /// Composes checking and linting for open documents over shared semantic state.
    ///
    /// # Errors
    /// Returns preparation or diagnostic failures.
    pub fn diagnostics(&mut self) -> io::Result<Vec<Diagnostic>> {
        self.prepare()?;

        self.collect()
    }

    /// Composes workspace diagnostics with cancellable per-module preparation.
    ///
    /// # Errors
    /// Returns preparation or diagnostic failures, or `Interrupted` when cancelled.
    pub fn workspace_diagnostics(
        &mut self,
        paths: &[PathBuf],
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> io::Result<Vec<Diagnostic>> {
        let open = self.open.clone();
        self.open.extend(paths.iter().cloned());

        let result = self
            .prepare_roots(paths, progress)
            .and_then(|()| self.collect());

        self.open = open;

        result
    }

    fn collect(&mut self) -> io::Result<Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();

        for session in &mut self.sessions {
            let open = self
                .open
                .intersection(&session.entries)
                .cloned()
                .collect::<BTreeSet<_>>();

            if open.is_empty() {
                continue;
            }

            diagnostics.extend(session.with_host(
                &mut self.project,
                Service::Lsp,
                Some(&open),
                |checker, host| {
                    check::collect(checker, host)?;
                    lint::collect(checker, host)?;

                    Ok(std::mem::take(&mut host.diagnostics))
                },
            )?);
        }

        diagnostic::sort(&mut diagnostics);

        Ok(diagnostics)
    }

    fn prepare_roots(
        &mut self,
        additional: &[PathBuf],
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> io::Result<()> {
        let cancelled = || io::Error::new(io::ErrorKind::Interrupted, "request cancelled");

        if !progress(0, 0) {
            return Err(cancelled());
        }

        for session in &mut self.sessions {
            session.entries.clear();
        }

        let paths = self
            .open
            .iter()
            .cloned()
            .chain(additional.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let mut environments = session::environments(&mut self.project, &paths, Service::Lsp)?;

        environments.sort_by(|a, b| {
            a.map
                .as_ref()
                .map(|map| &map.path)
                .cmp(&b.map.as_ref().map(|map| &map.path))
        });

        let total = environments
            .iter()
            .map(|environment| environment.entries.len())
            .sum();

        let mut completed = 0;

        for environment in environments {
            let declaration_paths = environment
                .entries
                .iter()
                .filter(|module| is_declaration_source(&module.source))
                .map(|module| module.source.clone())
                .collect::<BTreeSet<_>>();

            let index = self.sessions.iter().position(|session| {
                session.settings == environment.settings
                    && session.definitions == environment.definitions
                    && session.declaration_paths == declaration_paths
                    && session.map.as_ref().map(|map| &map.path)
                        == environment.map.as_ref().map(|map| &map.path)
            });

            let index = if let Some(index) = index {
                index
            } else {
                self.sessions.push(Session::new(
                    &mut self.project,
                    &environment,
                    Service::Lsp,
                    &CheckerOptions {
                        retain_full_type_graphs: 1,
                    },
                )?);

                self.sessions.len() - 1
            };

            self.sessions[index].prepare(
                &mut self.project,
                Service::Lsp,
                Some(&self.open),
                &environment.entries,
                &mut |_| {
                    if !progress(completed, total) {
                        return Err(cancelled());
                    }

                    completed += 1;

                    Ok(())
                },
            )?;

            if !progress(completed, total) {
                return Err(cancelled());
            }
        }

        Ok(())
    }

    /// Runs an editor query against a checked module in its first place context.
    ///
    /// # Errors
    /// Returns an error for unavailable modules or failed native queries.
    pub fn query<T>(
        &mut self,
        path: &Path,
        operation: impl FnOnce(&mut Checker, &mut dyn Callbacks, &str) -> io::Result<T>,
    ) -> io::Result<T> {
        let path = crate::absolute(path)?;

        for session in &mut self.sessions {
            let name = session
                .identities
                .iter()
                .filter(|(module, _)| module.source == path)
                .map(|(_, name)| name)
                .min()
                .cloned();

            if let Some(name) = name {
                return session.with_host(
                    &mut self.project,
                    Service::Lsp,
                    Some(&self.open),
                    |checker, host| operation(checker, host, &name),
                );
            }
        }

        Err(invalid(format!("{} has no checked module", path.display())))
    }

    /// Runs a query in every checked place and instance context of a source.
    ///
    /// # Errors
    /// Returns an error for unavailable modules or failed native queries.
    pub fn query_all<T>(
        &mut self,
        path: &Path,
        mut operation: impl FnMut(&mut Checker, &mut dyn Callbacks, &str) -> io::Result<T>,
    ) -> io::Result<Vec<T>> {
        let path = crate::absolute(path)?;
        let mut results = Vec::new();

        for session in &mut self.sessions {
            let mut names: Vec<_> = session
                .identities
                .iter()
                .filter(|(module, _)| module.source == path)
                .map(|(_, name)| name.clone())
                .collect();

            names.sort_unstable();
            names.dedup();

            for name in names {
                results.push(session.with_host(
                    &mut self.project,
                    Service::Lsp,
                    Some(&self.open),
                    |checker, host| operation(checker, host, &name),
                )?);
            }
        }

        if results.is_empty() {
            return Err(invalid(format!("{} has no checked module", path.display())));
        }

        Ok(results)
    }

    /// Returns native identities and source paths for all modules loaded in place contexts.
    #[must_use]
    pub fn module_identities(&self) -> Vec<(String, PathBuf)> {
        let mut identities = self
            .sessions
            .iter()
            .flat_map(|session| {
                session
                    .identities
                    .iter()
                    .map(|(module, name)| (name.clone(), module.source.clone()))
            })
            .collect::<Vec<_>>();

        identities.sort_unstable();
        identities.dedup();

        identities
    }

    /// Resolves a native identity to its physical source file.
    ///
    /// # Errors
    /// Returns an error for non-source identities such as built-in declarations.
    pub fn source_path(&self, name: &str) -> io::Result<PathBuf> {
        self.sessions
            .iter()
            .find_map(|session| session.modules.get(name))
            .map(|state| state.module.source.clone())
            .ok_or_else(|| invalid(format!("unknown source identity {name}")))
    }

    /// Reads the current overlay or disk source.
    ///
    /// # Errors
    /// Returns source-loading errors.
    pub fn source(&mut self, path: &Path) -> io::Result<Rc<str>> {
        self.project.source(path)
    }

    /// Drains asset warnings.
    pub fn take_asset_warnings(&mut self) -> Vec<String> {
        self.project.take_asset_warnings()
    }

    /// Lists services from the editor's cached platform declarations.
    ///
    /// # Errors
    /// Returns configuration or declaration-loading errors.
    pub fn services(&mut self, path: &Path) -> io::Result<Vec<String>> {
        self.project.services(path)
    }

    /// Loads merged documentation for a source's platform configuration.
    ///
    /// # Errors
    /// Returns documentation-loading errors.
    pub fn documentation(
        &mut self,
        path: &Path,
        symbol: &str,
    ) -> io::Result<Option<Rc<serde_json::Value>>> {
        self.project.documentation(path, symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;

    #[test]
    fn diagnostics_use_each_roots_declaration_environment() {
        let directory =
            std::env::temp_dir().join(format!("editor-environments-{}", std::process::id()));

        let nested = directory.join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        for (folder, kind) in [(&directory, "number"), (&nested, "string")] {
            std::fs::write(
                folder.join("instar.toml"),
                "[roblox]\nenabled = false\n[luau.definitions]\n\"@values\" = \"values.d.luau\"\n",
            )
            .unwrap();

            std::fs::write(
                folder.join("values.d.luau"),
                format!("declare value: {kind}\n"),
            )
            .unwrap();
        }

        let source = directory.join("main.luau");
        let dependency = nested.join("dependency.luau");
        std::fs::write(&source, "return require(\"./nested/dependency\")\n").unwrap();

        std::fs::write(
            &dependency,
            "--!strict\nlocal expected: string = value\nreturn expected\n",
        )
        .unwrap();

        let diagnostics = check::run(
            &mut Project::new(),
            std::slice::from_ref(&source),
            |_| {},
            |_| Ok(()),
        )
        .unwrap();

        assert!(
            errors(diagnostics)
                .iter()
                .any(|(path, _, _)| path == &dependency)
        );

        let mut editor = Editor::default();

        for path in [&source, &dependency] {
            editor
                .set_source(path, Some(&std::fs::read_to_string(path).unwrap()))
                .unwrap();
        }

        for _ in 0..2 {
            editor.prepare().unwrap();
            assert_eq!(errors(editor.diagnostics().unwrap()).len(), 0);
        }

        editor.set_source(&source, None).unwrap();
        assert_eq!(errors(editor.diagnostics().unwrap()).len(), 0);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn passes_share_semantics_and_compose_editor_diagnostics() {
        let directory =
            std::env::temp_dir().join(format!("diagnostic-modes-{}", std::process::id()));

        std::fs::create_dir_all(&directory).unwrap();
        let configuration = "[roblox]\nenabled = false\n[lint.luau]\n\"*\" = false\nFormatString = true\n[lint.rules]\nempty_if = \"deny\"\n";
        std::fs::write(directory.join("instar.toml"), configuration).unwrap();
        let source = directory.join("main.luau");
        let dependency = directory.join("dependency.luau");

        std::fs::write(
            &source,
            "--!strict\nlocal dependency = require(\"./dependency\")\nlocal value: number = \"text\"\nif true then end\nprint(dependency:match(\"[]\"))\nreturn value\n",
        ).unwrap();

        std::fs::write(&dependency, "if true then end\nreturn \"text\"\n").unwrap();

        let signature = |diagnostics: &[Diagnostic]| {
            let mut result = diagnostics.to_vec();
            diagnostic::sort(&mut result);

            result
        };

        let execute = |linting| {
            let mut reported = Vec::new();

            let report = |batch: &[Diagnostic]| {
                reported.extend(batch.iter().cloned());

                Ok(())
            };

            let result = if linting {
                lint::run(
                    &mut Project::new(),
                    std::slice::from_ref(&source),
                    |_| {},
                    report,
                )
            } else {
                check::run(
                    &mut Project::new(),
                    std::slice::from_ref(&source),
                    |_| {},
                    report,
                )
            }
            .unwrap();

            assert_eq!(signature(&reported), signature(&result));

            result
        };

        let types = execute(false);
        assert_eq!(types.len(), 1, "{types:?}");
        assert_eq!(types[0].location.range[0], 2);
        assert_eq!(types[0].severity, Severity::Error);
        let lints = execute(true);

        assert!(
            lints
                .iter()
                .any(|diagnostic| diagnostic.message.starts_with("FormatString:"))
        );

        assert!(lints.iter().any(|diagnostic| {
            diagnostic.location.module.source == dependency
                && diagnostic.message.starts_with("empty_if:")
                && diagnostic.severity == Severity::Error
        }));

        let mut expected = types;
        expected.extend(lints.iter().cloned());
        let mut editor = Editor::default();

        for path in [&source, &dependency] {
            editor
                .set_source(path, Some(&std::fs::read_to_string(path).unwrap()))
                .unwrap();
        }

        assert_eq!(
            signature(&editor.diagnostics().unwrap()),
            signature(&expected)
        );

        std::fs::write(
            directory.join("instar.toml"),
            format!("{configuration}\n[check]\nexclude = [\"main.luau\"]\n"),
        )
        .unwrap();

        assert_eq!(execute(false).len(), 0);
        assert_eq!(signature(&execute(true)), signature(&lints));

        std::fs::write(
            directory.join("instar.toml"),
            configuration.replace(
                "[lint.luau]",
                "[lint]\nexclude = [\"dependency.luau\"]\n[lint.luau]",
            ),
        )
        .unwrap();

        let selected = execute(true);

        assert!(
            selected
                .iter()
                .all(|diagnostic| diagnostic.location.module.source == source)
        );

        assert!(
            selected
                .iter()
                .any(|diagnostic| diagnostic.message.starts_with("FormatString:"))
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    fn declaration_fixture(name: &str, filename: &str) -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        std::fs::write(
            directory.join("instar.toml"),
            format!(
                "[roblox]\nenabled = true\nsourcemaps = []\n[luau.definitions]\n\"@roblox\" = \"{filename}\"\n"
            ),
        )
        .unwrap();

        let source = directory.join("main.luau");

        std::fs::write(
            &source,
            "--!strict\nlocal item: Part = Instance.new(\"Part\")\nlocal value: string = marker\nreturn item, value\n",
        )
        .unwrap();

        (source, directory.join(filename))
    }

    fn declaration(metadata: &str, marker: &str) -> String {
        format!(
            "--#METADATA#{metadata}\ndeclare extern type Instance with\nend\ndeclare extern type Part extends Instance with\nend\ndeclare Instance: {{ new: (className: string) -> Instance }}\ndeclare marker: {marker}\n"
        )
    }

    fn has_errors(diagnostics: &[Diagnostic]) -> bool {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    fn errors(diagnostics: Vec<Diagnostic>) -> Vec<(PathBuf, [u32; 4], String)> {
        let mut errors = diagnostics
            .into_iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| {
                (
                    diagnostic.location.module.source,
                    diagnostic.location.range,
                    diagnostic.message,
                )
            })
            .collect::<Vec<_>>();

        errors.sort();

        errors
    }

    #[test]
    fn editor_requires_follow_overlay_existence_and_content() {
        let directory =
            std::env::temp_dir().join(format!("editor-overlays-{}", std::process::id()));

        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("instar.toml"), "[roblox]\nenabled = false\n").unwrap();
        let source = directory.join("main.luau");
        let dependency = directory.join("missing.luau");
        let mut editor = Editor::default();

        editor
            .set_source(
                &source,
                Some("--!strict\nlocal value: number = require(\"./missing\")\nreturn value\n"),
            )
            .unwrap();

        let missing = errors(editor.diagnostics().unwrap());
        assert!(missing.iter().any(|(path, _, _)| path == &source));
        assert_eq!(errors(editor.diagnostics().unwrap()), missing);

        editor.set_source(&dependency, Some("return 1\n")).unwrap();
        editor.prepare().unwrap();

        for _ in 0..2 {
            assert_eq!(
                errors(editor.diagnostics().unwrap()),
                Vec::<(PathBuf, [u32; 4], String)>::new()
            );
        }

        editor
            .set_source(&dependency, Some("return \"text\"\n"))
            .unwrap();

        let incompatible = errors(editor.diagnostics().unwrap());
        assert!(incompatible.iter().any(|(path, _, _)| path == &source));
        assert_eq!(errors(editor.diagnostics().unwrap()), incompatible);

        editor.set_source(&dependency, Some("return 2\n")).unwrap();

        for _ in 0..2 {
            assert_eq!(
                errors(editor.diagnostics().unwrap()),
                Vec::<(PathBuf, [u32; 4], String)>::new()
            );
        }

        editor.set_source(&dependency, None).unwrap();
        editor.prepare().unwrap();

        for _ in 0..2 {
            assert_eq!(errors(editor.diagnostics().unwrap()), missing);
        }

        editor.set_source(&dependency, Some("return 1\n")).unwrap();

        assert_eq!(
            errors(editor.diagnostics().unwrap()),
            Vec::<(PathBuf, [u32; 4], String)>::new()
        );

        std::fs::write(&dependency, "return 1\n").unwrap();
        editor.set_source(&dependency, None).unwrap();

        assert_eq!(
            errors(editor.diagnostics().unwrap()),
            Vec::<(PathBuf, [u32; 4], String)>::new()
        );

        editor
            .set_source(&dependency, Some("return \"text\"\n"))
            .unwrap();

        assert_eq!(errors(editor.diagnostics().unwrap()), incompatible);
        editor.set_source(&dependency, None).unwrap();

        for _ in 0..2 {
            assert_eq!(
                errors(editor.diagnostics().unwrap()),
                Vec::<(PathBuf, [u32; 4], String)>::new()
            );
        }

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn editor_declaration_diagnostics_survive_queries_and_repeated_checks() {
        let directory =
            std::env::temp_dir().join(format!("editor-declaration-errors-{}", std::process::id()));

        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("instar.toml"), "[roblox]\nenabled = false\n").unwrap();
        let source = directory.join("main.luau");
        let definitions = directory.join("types.d.luau");
        let valid = "declare marker: string\n";
        let invalid = "declare marker:\n";
        std::fs::write(&definitions, valid).unwrap();
        let mut editor = Editor::default();
        editor.set_source(&source, Some("return 1\n")).unwrap();
        editor.set_source(&definitions, Some(invalid)).unwrap();
        editor.prepare().unwrap();

        let expected = errors(editor.diagnostics().unwrap());
        assert!(expected.iter().any(|(path, _, _)| path == &definitions));

        for _ in 0..2 {
            editor.prepare().unwrap();
            assert_eq!(errors(editor.diagnostics().unwrap()), expected);
        }

        editor.refresh();
        editor.prepare().unwrap();
        assert_eq!(errors(editor.diagnostics().unwrap()), expected);

        editor.set_source(&definitions, Some(valid)).unwrap();

        for _ in 0..2 {
            assert_eq!(
                errors(editor.diagnostics().unwrap()),
                Vec::<(PathBuf, [u32; 4], String)>::new()
            );
        }

        editor.set_source(&definitions, Some(invalid)).unwrap();
        assert_eq!(errors(editor.diagnostics().unwrap()), expected);
        editor.set_source(&definitions, None).unwrap();

        for _ in 0..2 {
            assert_eq!(
                errors(editor.diagnostics().unwrap()),
                Vec::<(PathBuf, [u32; 4], String)>::new()
            );
        }

        editor.set_source(&definitions, Some(invalid)).unwrap();
        editor.prepare().unwrap();
        assert_eq!(errors(editor.diagnostics().unwrap()), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn editor_declarations_follow_effective_source_through_close_and_refresh() {
        let (source, definitions) = declaration_fixture("editor-declarations", "types.luau");
        let disk = declaration(r#"{"services":[],"creatable_instances":[]}"#, "number");

        let overlay = declaration(
            r#"{"services":["Part"],"creatable_instances":["Part"]}"#,
            "string",
        );

        std::fs::write(&definitions, &disk).unwrap();
        let mut editor = Editor::default();

        editor
            .set_source(&source, Some(&std::fs::read_to_string(&source).unwrap()))
            .unwrap();

        assert_eq!(editor.services(&source).unwrap(), Vec::<String>::new());
        assert!(has_errors(&editor.diagnostics().unwrap()));

        editor.set_source(&definitions, Some(&overlay)).unwrap();

        for refresh in [false, false, true] {
            if refresh {
                editor.refresh();
            }

            assert_eq!(editor.services(&source).unwrap(), ["Part"]);
            assert!(!has_errors(&editor.diagnostics().unwrap()));
        }

        let malformed = declaration(
            r#"{"services":["Part","Part"],"creatable_instances":["Part"]}"#,
            "string",
        );

        editor.set_source(&definitions, Some(&malformed)).unwrap();
        assert!(editor.services(&source).is_err());
        assert!(editor.diagnostics().is_err());
        editor.set_source(&definitions, Some(&overlay)).unwrap();
        assert_eq!(editor.services(&source).unwrap(), ["Part"]);
        assert!(!has_errors(&editor.diagnostics().unwrap()));

        editor.set_source(&definitions, None).unwrap();
        assert_eq!(editor.services(&source).unwrap(), Vec::<String>::new());
        assert!(has_errors(&editor.diagnostics().unwrap()));
        std::fs::write(&definitions, &overlay).unwrap();
        editor.refresh();
        assert_eq!(editor.services(&source).unwrap(), ["Part"]);
        assert!(!has_errors(&editor.diagnostics().unwrap()));
        std::fs::remove_dir_all(source.parent().unwrap()).unwrap();
    }

    #[test]
    fn batch_declarations_accept_overlay_only_files() {
        let (source, definitions) = declaration_fixture("batch-declarations", "types.d.luau");

        let overlay = declaration(
            r#"{"services":["Part"],"creatable_instances":["Part"]}"#,
            "string",
        );

        let mut project = Project::new();
        project.set_source(&definitions, Some(&overlay)).unwrap();
        assert_eq!(project.services(&source).unwrap(), ["Part"]);

        assert!(!has_errors(
            &check::run(
                &mut project,
                &[source.clone(), definitions.clone()],
                |_| {},
                |_| Ok(())
            )
            .unwrap()
        ));

        project.set_source(&definitions, None).unwrap();

        assert!(
            check::run(
                &mut project,
                std::slice::from_ref(&source),
                |_| {},
                |_| Ok(())
            )
            .is_err()
        );

        std::fs::remove_dir_all(source.parent().unwrap()).unwrap();
    }

    #[test]
    fn streaming_reports_each_diagnostic_once() {
        let path = std::env::temp_dir().join(format!("streaming-{}.luau", std::process::id()));

        std::fs::write(&path, "--!strict\nlocal value: number = \"text\"\n").unwrap();

        let mut reported = Vec::new();
        let mut seen = Vec::new();

        let result = check::run(
            &mut Project::new(),
            std::slice::from_ref(&path),
            |module| {
                seen.push(module.to_owned());
            },
            |batch| {
                reported.extend(batch.iter().map(|diagnostic| {
                    (
                        diagnostic.location.range,
                        diagnostic.severity,
                        diagnostic.message.clone(),
                    )
                }));

                Ok(())
            },
        )
        .unwrap();

        assert!(seen.contains(&path));
        std::fs::remove_file(path).unwrap();

        let mut returned = result
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.location.range,
                    diagnostic.severity,
                    diagnostic.message.clone(),
                )
            })
            .collect::<Vec<_>>();

        assert!(
            returned
                .iter()
                .any(|(_, severity, _)| *severity == Severity::Error)
        );

        reported.sort();
        returned.sort();
        assert_eq!(reported, returned);
    }
}
