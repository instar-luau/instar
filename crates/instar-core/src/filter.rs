//! Shared file selection. Globs are case-sensitive; `*` stays within a directory,
//! while `**` crosses directories. Exclusions do not prevent loading dependencies.

use std::{collections::BTreeMap, io, path::Path, rc::Rc};

use glob::{MatchOptions, Pattern};

use crate::{absolute, config::Config, invalid};

/// Service whose file selection is intersected with global rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Service {
    /// Type analysis.
    Analyze,

    /// Compiled output.
    Build,

    /// Source formatting.
    Format,

    /// Dependency installation.
    Graft,

    /// Lint diagnostics.
    Lint,

    /// Language server.
    Lsp,
}

#[derive(Clone)]
struct Rule {
    directory: Option<Rc<Path>>,
    pattern: Pattern,
}

impl Rule {
    fn matches(&self, path: &Path) -> bool {
        let path = match &self.directory {
            Some(directory) => match path.strip_prefix(directory) {
                Ok(path) => path,
                Err(_) => return false,
            },

            None => path,
        };

        self.pattern.matches_path_with(
            path,
            MatchOptions {
                case_sensitive: true,
                require_literal_separator: true,
                require_literal_leading_dot: false,
            },
        )
    }
}

#[derive(Clone, Default)]
struct Scope {
    include: Vec<Rc<Rule>>,
    exclude: Vec<Rc<Rule>>,
}

impl Scope {
    fn append(
        &mut self,
        directory: &Path,
        include: &[String],
        exclude: &[String],
    ) -> io::Result<()> {
        let directory: Rc<Path> = directory.into();

        for (rules, patterns) in [(&mut self.include, include), (&mut self.exclude, exclude)] {
            for pattern in patterns {
                if pattern.is_empty() || pattern.contains('\0') {
                    return Err(invalid("file globs must be nonempty and contain no NUL"));
                }

                let rooted = absolute(&directory.join(pattern))?;

                let (base, relative) = match rooted.strip_prefix(&directory) {
                    Ok(relative) => (Some(Rc::clone(&directory)), relative),
                    Err(_) => (None, rooted.as_path()),
                };

                let text = relative
                    .to_str()
                    .ok_or_else(|| invalid("file glob is not UTF-8"))?;

                let compiled = Pattern::new(text)
                    .map_err(|error| invalid(format!("invalid file glob {pattern:?}: {error}")))?;

                rules.push(Rc::new(Rule {
                    directory: base,
                    pattern: compiled,
                }));
            }
        }

        Ok(())
    }

    fn includes(&self, path: &Path) -> bool {
        (self.include.is_empty() || self.include.iter().any(|rule| rule.matches(path)))
            && !self.exclude.iter().any(|rule| rule.matches(path))
    }
}

#[derive(Clone, Default)]
pub(crate) struct Filters {
    global: Scope,
    services: BTreeMap<Service, Scope>,
}

impl Filters {
    pub(crate) fn append(&mut self, directory: &Path, config: &Config) -> io::Result<()> {
        self.global
            .append(directory, &config.include, &config.exclude)?;

        for (service, include, exclude) in [
            (
                Service::Analyze,
                &config.analyze.include,
                &config.analyze.exclude,
            ),
            (Service::Build, &config.build.include, &config.build.exclude),
            (
                Service::Format,
                &config.format.include,
                &config.format.exclude,
            ),
            (Service::Graft, &config.graft.include, &config.graft.exclude),
            (Service::Lint, &config.lint.include, &config.lint.exclude),
            (Service::Lsp, &config.lsp.include, &config.lsp.exclude),
        ] {
            if !include.is_empty() || !exclude.is_empty() {
                self.services
                    .entry(service)
                    .or_default()
                    .append(directory, include, exclude)?;
            }
        }

        Ok(())
    }

    pub(crate) fn includes(&self, path: &Path, service: Service) -> bool {
        self.global.includes(path)
            && self
                .services
                .get(&service)
                .is_none_or(|scope| scope.includes(path))
    }
}
