//! Shared file selection. Globs are case-sensitive; `*` stays within a directory,
//! while `**` crosses directories. Exclusions do not prevent loading dependencies.

use std::{collections::BTreeMap, io, path::Path, rc::Rc};

use glob::{MatchOptions, Pattern};

use crate::{absolute, config::Config, invalid};

/// Operation whose file selection falls back to global fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Service {
    /// Syntax and type checking.
    Check,

    /// Source formatting.
    Format,

    /// Lint diagnostics.
    Lint,

    /// Language server workspace indexing.
    Index,

    /// Autoimport candidates, independent of workspace indexing.
    Imports,
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
    include: Option<Vec<Rc<Rule>>>,
    exclude: Option<Vec<Rc<Rule>>>,
}

impl Scope {
    fn merge(
        &mut self,
        directory: &Path,
        include: Option<&[String]>,
        exclude: Option<&[String]>,
    ) -> io::Result<()> {
        let directory: Rc<Path> = directory.into();

        for (rules, patterns) in [(&mut self.include, include), (&mut self.exclude, exclude)] {
            let Some(patterns) = patterns else {
                continue;
            };

            let mut compiled_rules = Vec::with_capacity(patterns.len());

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

                compiled_rules.push(Rc::new(Rule {
                    directory: base,
                    pattern: compiled,
                }));
            }

            *rules = Some(compiled_rules);
        }

        Ok(())
    }

    fn includes(&self, path: &Path, fallback: &Self) -> bool {
        let include = self.include.as_ref().or(fallback.include.as_ref());
        let exclude = self.exclude.as_ref().or(fallback.exclude.as_ref());

        include.is_none_or(|rules| rules.is_empty() || rules.iter().any(|rule| rule.matches(path)))
            && exclude.is_none_or(|rules| !rules.iter().any(|rule| rule.matches(path)))
    }
}

#[derive(Clone, Default)]
pub(crate) struct Filters {
    global: Scope,
    services: BTreeMap<Service, Scope>,
}

impl Filters {
    pub(crate) fn merge(&mut self, directory: &Path, config: &Config) -> io::Result<()> {
        self.global.merge(
            directory,
            config.include.as_deref(),
            config.exclude.as_deref(),
        )?;

        for (service, include, exclude) in [
            (Service::Check, &config.check.include, &config.check.exclude),
            (
                Service::Format,
                &config.format.include,
                &config.format.exclude,
            ),
            (Service::Lint, &config.lint.include, &config.lint.exclude),
            (
                Service::Index,
                &config.lsp.index.include,
                &config.lsp.index.exclude,
            ),
            (
                Service::Imports,
                &config.lsp.imports.include,
                &config.lsp.imports.exclude,
            ),
        ] {
            if include.is_some() || exclude.is_some() {
                self.services.entry(service).or_default().merge(
                    directory,
                    include.as_deref(),
                    exclude.as_deref(),
                )?;
            }
        }

        Ok(())
    }

    pub(crate) fn includes(&self, path: &Path, service: Service) -> bool {
        self.services
            .get(&service)
            .unwrap_or(&self.global)
            .includes(path, &self.global)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_fields_inherit_and_empty_lists_clear_independently() {
        let root = absolute(Path::new("project")).unwrap();

        let parent: Config = toml::from_str(
            r#"
include = ["src/**"]
exclude = ["src/private/**"]
[check]
include = ["selected/**"]
exclude = ["selected/private/**"]
"#,
        )
        .unwrap();

        let mut filters = Filters::default();
        filters.merge(&root, &parent).unwrap();

        filters
            .merge(&root.join("child"), &Config::default())
            .unwrap();

        assert!(filters.includes(&root.join("selected/main.luau"), Service::Check));
        assert!(!filters.includes(&root.join("outside.luau"), Service::Check));
        assert!(!filters.includes(&root.join("selected/private/init.luau"), Service::Check));

        let child: Config = toml::from_str("[check]\ninclude = []").unwrap();
        filters.merge(&root.join("child"), &child).unwrap();
        assert!(filters.includes(&root.join("outside.luau"), Service::Check));
        assert!(!filters.includes(&root.join("selected/private/init.luau"), Service::Check));

        let child: Config = toml::from_str("[check]\nexclude = []").unwrap();
        filters.merge(&root.join("child"), &child).unwrap();
        assert!(filters.includes(&root.join("selected/private/init.luau"), Service::Check));

        let child: Config = toml::from_str("include = []").unwrap();
        filters.merge(&root.join("child"), &child).unwrap();
        assert!(filters.includes(&root.join("outside.luau"), Service::Format));
        assert!(!filters.includes(&root.join("src/private/init.luau"), Service::Format));

        let child: Config = toml::from_str("exclude = []").unwrap();
        filters.merge(&root.join("child"), &child).unwrap();
        assert!(filters.includes(&root.join("src/private/init.luau"), Service::Format));
    }

    #[test]
    fn service_filters_replace_global_fallback_without_affecting_other_services() {
        let config: Config = toml::from_str(
            r#"
include = ["global/**"]
exclude = ["**"]
[check]
include = ["check/**"]
exclude = ["check/private/**"]
[lint]
include = ["lint/**"]
exclude = ["lint/private/**"]
[format]
include = ["format/**"]
exclude = ["format/private/**"]
[lsp.index]
include = ["index/**"]
exclude = ["index/private/**"]
[lsp.imports]
include = ["imports/**"]
exclude = ["imports/private/**"]
"#,
        )
        .unwrap();

        let root = absolute(Path::new("project")).unwrap();
        let mut filters = Filters::default();
        filters.merge(&root, &config).unwrap();

        let services = [
            (Service::Check, "check"),
            (Service::Lint, "lint"),
            (Service::Format, "format"),
            (Service::Index, "index"),
            (Service::Imports, "imports"),
        ];

        for (service, selected) in services {
            for (_, directory) in services {
                assert_eq!(
                    filters.includes(&root.join(directory).join("main.luau"), service),
                    selected == directory,
                    "{service:?}: {directory}"
                );
            }

            assert!(!filters.includes(&root.join(selected).join("private/init.luau"), service));
        }
    }

    #[test]
    fn inherited_and_replaced_fields_keep_their_defining_glob_origins() {
        let root = absolute(Path::new("project")).unwrap();

        let parent: Config = toml::from_str(
            "[check]\ninclude = [\"shared/**\", \"place/**\"]\nexclude = [\"shared/private/**\"]",
        )
        .unwrap();

        let child: Config =
            toml::from_str("[check]\ninclude = [\"../shared/**\", \"local/**\"]").unwrap();

        let mut filters = Filters::default();
        filters.merge(&root, &parent).unwrap();
        filters.merge(&root.join("place"), &child).unwrap();

        filters
            .merge(&root.join("place/nested"), &Config::default())
            .unwrap();

        for (path, included) in [
            ("shared/main.luau", true),
            ("shared/private/init.luau", false),
            ("place/local/main.luau", true),
            ("place/main.luau", false),
            ("place/nested/local/main.luau", false),
            ("local/main.luau", false),
        ] {
            assert_eq!(
                filters.includes(&root.join(path), Service::Check),
                included,
                "{path}"
            );
        }

        let descendant: Config =
            toml::from_str("[check]\nexclude = [\"../../shared/child-private/**\"]").unwrap();

        filters
            .merge(&root.join("place/nested"), &descendant)
            .unwrap();

        assert!(filters.includes(&root.join("shared/private/init.luau"), Service::Check));
        assert!(!filters.includes(&root.join("shared/child-private/init.luau"), Service::Check));
        assert!(filters.includes(&root.join("place/local/main.luau"), Service::Check));
    }

    #[test]
    fn descendant_globals_update_only_operation_fields_using_fallback() {
        let root = absolute(Path::new("project")).unwrap();

        let parent: Config = toml::from_str(
            r#"
include = ["src/**"]
exclude = ["blocked/**"]
[check]
include = ["custom/**"]
[lsp.imports]
exclude = ["src/private/**", "place/lib/private/**"]
"#,
        )
        .unwrap();

        let child: Config =
            toml::from_str("include = [\"lib/**\"]\nexclude = [\"../custom/private/**\"]").unwrap();

        let mut filters = Filters::default();
        filters.merge(&root, &parent).unwrap();
        filters.merge(&root.join("place"), &child).unwrap();

        for service in [
            Service::Format,
            Service::Lint,
            Service::Index,
            Service::Imports,
        ] {
            assert!(filters.includes(&root.join("place/lib/main.luau"), service));
            assert!(!filters.includes(&root.join("src/main.luau"), service));
        }

        assert!(filters.includes(&root.join("custom/main.luau"), Service::Check));
        assert!(!filters.includes(&root.join("custom/private/init.luau"), Service::Check));
        assert!(!filters.includes(&root.join("place/lib/private/init.luau"), Service::Imports));
        assert!(filters.includes(&root.join("place/lib/private/init.luau"), Service::Index));

        let child: Config = toml::from_str("include = []\nexclude = []").unwrap();
        filters.merge(&root.join("place"), &child).unwrap();
        assert!(filters.includes(&root.join("custom/private/init.luau"), Service::Check));
        assert!(!filters.includes(&root.join("outside.luau"), Service::Check));
        assert!(filters.includes(&root.join("outside.luau"), Service::Imports));
        assert!(!filters.includes(&root.join("place/lib/private/init.luau"), Service::Imports));
    }

    #[test]
    fn imports_resolve_their_own_fields_independently_of_indexing() {
        let root = absolute(Path::new("project")).unwrap();

        let parent: Config = toml::from_str(
            r#"
include = ["modules/**"]
exclude = ["modules/global/**"]
[lsp.index]
include = ["workspace/**"]
exclude = ["workspace/private/**"]
"#,
        )
        .unwrap();

        let mut filters = Filters::default();
        filters.merge(&root, &parent).unwrap();
        assert!(filters.includes(&root.join("modules/main.luau"), Service::Imports));
        assert!(!filters.includes(&root.join("modules/main.luau"), Service::Index));
        assert!(!filters.includes(&root.join("modules/global/init.luau"), Service::Imports));
        assert!(filters.includes(&root.join("workspace/main.luau"), Service::Index));
        assert!(!filters.includes(&root.join("workspace/main.luau"), Service::Imports));

        let child: Config =
            toml::from_str("[lsp.imports]\ninclude = [\"../modules/**\"]\nexclude = []").unwrap();

        filters.merge(&root.join("place"), &child).unwrap();
        assert!(filters.includes(&root.join("modules/global/init.luau"), Service::Imports));
        assert!(!filters.includes(&root.join("modules/global/init.luau"), Service::Index));
        assert!(!filters.includes(&root.join("workspace/private/init.luau"), Service::Index));

        let child: Config = toml::from_str("[lsp.index]\ninclude = []\nexclude = []").unwrap();
        filters.merge(&root.join("place"), &child).unwrap();
        assert!(filters.includes(&root.join("workspace/private/init.luau"), Service::Index));
        assert!(!filters.includes(&root.join("workspace/private/init.luau"), Service::Imports));
        assert!(filters.includes(&root.join("modules/global/init.luau"), Service::Imports));
    }

    #[test]
    fn invalid_globs_are_validated_in_every_filter_field() {
        let root = absolute(Path::new("project")).unwrap();

        for table in ["", "check", "format", "lint", "lsp.index", "lsp.imports"] {
            for field in ["include", "exclude"] {
                for pattern in ["", "\0", "["] {
                    let mut value = serde_json::json!({});
                    let mut object = &mut value;

                    for part in table.split('.').filter(|part| !part.is_empty()) {
                        object = object
                            .as_object_mut()
                            .unwrap()
                            .entry(part.to_owned())
                            .or_insert_with(|| serde_json::json!({}));
                    }

                    object[field] = serde_json::json!([pattern]);
                    let config: Config = serde_json::from_value(value).unwrap();
                    let error = Filters::default().merge(&root, &config).unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                }
            }
        }
    }
}
