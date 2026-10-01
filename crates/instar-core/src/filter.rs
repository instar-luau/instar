//! Shared file selection. Globs are case-sensitive; `*` stays within a directory,
//! while `**` crosses directories. Exclusions do not prevent loading dependencies.

use std::{collections::BTreeMap, io, path::Path, rc::Rc};

use glob::{MatchOptions, Pattern};

use crate::{absolute, config::Config, invalid};

/// Service whose file selection is intersected with global rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Service {
    /// Syntax and type checking.
    Check,

    /// Source formatting.
    Format,

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

    fn excludes_subtree(&self, directory: &Path) -> bool {
        let Some(base) = self.directory.as_deref() else {
            return false;
        };

        let Some(prefix) = self.pattern.as_str().strip_suffix("/**").or_else(|| {
            if cfg!(windows) {
                self.pattern.as_str().strip_suffix("\\**")
            } else {
                None
            }
        }) else {
            return false;
        };

        !prefix.is_empty()
            && !prefix.bytes().any(|byte| {
                matches!(byte, b'*' | b'?' | b'[' | b']') || (byte == b'\\' && !cfg!(windows))
            })
            && directory
                .strip_prefix(base)
                .is_ok_and(|relative| relative.starts_with(prefix))
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

    fn excludes_subtree(&self, directory: &Path) -> bool {
        self.exclude
            .iter()
            .any(|rule| rule.excludes_subtree(directory))
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
            (Service::Check, &config.check.include, &config.check.exclude),
            (
                Service::Format,
                &config.format.include,
                &config.format.exclude,
            ),
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

    pub(crate) fn excludes_subtree(&self, directory: &Path, service: Service) -> bool {
        self.global.excludes_subtree(directory)
            || self
                .services
                .get(&service)
                .is_some_and(|scope| scope.excludes_subtree(directory))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_and_lint_selection_are_independent() {
        let config: Config = toml::from_str(
            r#"
include = ["src/**"]
exclude = ["src/shared/global-skip.luau"]

[check]
include = ["src/check/**", "src/shared/**"]
exclude = ["src/shared/lint-only.luau"]

[lint]
include = ["src/lint/**", "src/shared/**"]
exclude = ["src/shared/check-only.luau"]
"#,
        )
        .unwrap();

        let root = crate::absolute(Path::new("project")).unwrap();
        let mut filters = Filters::default();
        filters.append(&root, &config).unwrap();

        for (path, check, lint) in [
            ("src/check/only.luau", true, false),
            ("src/lint/only.luau", false, true),
            ("src/shared/check-only.luau", true, false),
            ("src/shared/lint-only.luau", false, true),
            ("src/shared/both.luau", true, true),
            ("src/shared/global-skip.luau", false, false),
            ("outside.luau", false, false),
        ] {
            let path = root.join(path);
            assert_eq!(filters.includes(&path, Service::Check), check);
            assert_eq!(filters.includes(&path, Service::Lint), lint);
        }
    }

    #[test]
    fn prunes_only_literal_recursive_exclusions() {
        let root: Rc<Path> = Rc::from(Path::new("project"));

        let rule = Rule {
            directory: Some(Rc::clone(&root)),
            pattern: Pattern::new("packages/**").unwrap(),
        };

        assert!(rule.excludes_subtree(Path::new("project/packages")));
        assert!(rule.excludes_subtree(Path::new("project/packages/nested")));
        assert!(!rule.excludes_subtree(Path::new("project/packages-extra")));

        let wildcard = Rule {
            directory: Some(root),
            pattern: Pattern::new("packages/*/**").unwrap(),
        };

        assert!(!wildcard.excludes_subtree(Path::new("project/packages/module")));
        let mut filters = Filters::default();

        let config = Config {
            exclude: vec!["packages/**".to_owned()],
            ..Config::default()
        };

        let root = crate::absolute(Path::new("project")).unwrap();
        filters.append(&root, &config).unwrap();
        assert!(filters.excludes_subtree(&root.join("packages"), Service::Format));
    }
}
