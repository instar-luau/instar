use std::{collections::BTreeSet, fs, io, path::Path};

use instar_analysis::error::invalid;
use instar_bridge::frontend::{Notation, Parentheses, Suggestion, SuggestionKind};

use crate::{
    project::Project,
    resolve::{Module, Request},
    source::Kind,
};

impl Project {
    pub(crate) fn imports(&mut self, module: &Module, prefix: &str) -> io::Result<Vec<Suggestion>> {
        let directory = module
            .source
            .parent()
            .ok_or_else(|| invalid("source has no parent"))?;

        let settings = self.configuration(directory)?;
        let mut suggestions = Vec::new();

        if settings.configuration.roblox.enabled == Some(true) {
            match self.mapped_imports(module, prefix) {
                Ok(mapped) => suggestions.extend(mapped),
                Err(error) if unavailable(&error) => {}
                Err(error) => return Err(error),
            }

            if prefix.replace('\\', "/").starts_with("@game/") {
                return Ok(suggestions);
            }
        }

        let names = self.import_names(module, prefix)?;

        for name in names {
            self.interrupted()?;

            let path = match self.import_path(module, &name) {
                Ok(path) => path,
                Err(error) if unavailable(&error) => continue,
                Err(error) => return Err(error),
            };

            let directory = self.view.kind(&path)? == Some(Kind::Directory);

            let source = match self.probe(&path) {
                Ok(source) => source,
                Err(error) if unavailable(&error) => continue,
                Err(error) => return Err(error),
            };

            if directory {
                suggestions.push(suggestion(format!("{}/", name.trim_end_matches('/')), true));
            }

            if source.is_none() {
                continue;
            }

            match self.resolve(module, &Request::String(name.clone())).result {
                Ok(_) => suggestions.push(suggestion(name, false)),
                Err(error) if unavailable(&error.error()) => {}
                Err(error) => return Err(error.error()),
            }
        }

        suggestions.sort_by(|left, right| left.name.cmp(&right.name));
        suggestions.dedup_by(|left, right| left.name == right.name);

        Ok(suggestions)
    }

    fn import_names(&mut self, module: &Module, prefix: &str) -> io::Result<BTreeSet<String>> {
        let prefix = prefix.replace('\\', "/");
        let mut names = BTreeSet::new();

        for name in ["./", "../"] {
            if name.starts_with(&prefix) {
                names.insert(name.to_owned());
            }
        }

        if !prefix.contains('/') {
            let directory = module
                .source
                .parent()
                .ok_or_else(|| invalid("source has no parent"))?;

            let settings = self.configuration(directory)?;

            for alias in settings
                .snapshot
                .aliases
                .keys()
                .map(String::as_str)
                .chain(["self"])
            {
                self.interrupted()?;
                let name = format!("@{alias}");

                if name.starts_with(&prefix) {
                    names.insert(name);
                }
            }
        }

        let base = if prefix.is_empty() {
            Some("./")
        } else {
            prefix.rfind('/').map(|end| &prefix[..=end])
        };

        if let Some(base) = base {
            match self.import_path(module, base) {
                Ok(directory) if self.view.kind(&directory)? == Some(Kind::Directory) => {
                    let mut paths = self.view.snapshot.children(&directory);

                    match fs::read_dir(&directory) {
                        Ok(entries) => {
                            for entry in entries {
                                self.interrupted()?;
                                paths.insert(entry?.path());
                            }
                        }

                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }

                    for path in paths {
                        self.interrupted()?;
                        let kind = self.view.kind(&path)?;

                        let name = match kind {
                            Some(Kind::Directory) => path.file_name(),

                            Some(Kind::File)
                                if matches!(
                                    path.extension().and_then(|extension| extension.to_str()),
                                    Some("lua" | "luau")
                                ) =>
                            {
                                path.file_stem()
                            }

                            _ => continue,
                        };

                        let Some(name) = name.and_then(|name| name.to_str()) else {
                            continue;
                        };

                        if matches!(name, ".config" | "init") && kind == Some(Kind::File) {
                            continue;
                        }

                        let name = format!("{base}{name}");

                        if name.starts_with(&prefix) {
                            names.insert(name);
                        }
                    }
                }

                Ok(_) => {}
                Err(error) if unavailable(&error) => {}
                Err(error) => return Err(error),
            }
        }

        Ok(names)
    }

    fn mapped_imports(&mut self, module: &Module, prefix: &str) -> io::Result<Vec<Suggestion>> {
        let prefix = prefix.replace('\\', "/");

        if !prefix.starts_with("@game/") && !"@game/".starts_with(&prefix) {
            return Ok(Vec::new());
        }

        let map = self.context_map(module)?;
        let mut node = map.root()?;

        if "@game/".starts_with(&prefix) && prefix != "@game/" {
            return Ok(vec![suggestion("@game/".to_owned(), true)]);
        }

        let end = prefix
            .rfind('/')
            .ok_or_else(|| invalid("mapped import has no separator"))?;

        let base = &prefix[..=end];

        for component in base["@game/".len()..].split('/') {
            self.interrupted()?;

            node = match component {
                "" | "." => node,
                ".." => map.parent(node)?,
                name => map.child(node, name, false)?,
            };
        }

        let mut suggestions = Vec::new();
        let names = map.children(node).collect::<BTreeSet<_>>();

        for name in names {
            self.interrupted()?;

            if name.is_empty()
                || name.contains('/')
                || name.contains('\\')
                || matches!(name, "." | "..")
            {
                continue;
            }

            let child = match map.child(node, name, false) {
                Ok(child) => child,
                Err(error) if unavailable(&error) => continue,
                Err(error) => return Err(error),
            };

            let name = format!("{base}{name}");

            if !name.starts_with(&prefix) {
                continue;
            }

            if map.children(child).next().is_some() {
                suggestions.push(suggestion(format!("{name}/"), true));
            }

            match self.resolve(module, &Request::String(name.clone())).result {
                Ok(_) => suggestions.push(suggestion(name, false)),
                Err(error) if unavailable(&error.error()) => {}
                Err(error) => return Err(error.error()),
            }
        }

        suggestions.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(suggestions)
    }
}

pub(super) fn relative(source: &Path, target: &Path) -> Option<String> {
    let source = source.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();

    let common = source
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();

    if common == 0 {
        return None;
    }

    let mut components = vec![".."; source.len() - common];

    for component in &target[common..] {
        components.push(component.as_os_str().to_str()?);
    }

    let path = components.join("/");

    Some(if source.len() == common {
        format!("./{path}")
    } else {
        path
    })
}

pub(super) fn unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::InvalidInput
            | io::ErrorKind::InvalidData
            | io::ErrorKind::NotFound
            | io::ErrorKind::NotADirectory
    )
}

fn suggestion(name: String, directory: bool) -> Suggestion {
    Suggestion {
        name,
        kind: SuggestionKind::Path,
        detail: if directory { "directory" } else { "module" }.to_owned(),
        documentation: None,
        insert_text: None,
        parentheses: Parentheses::None,
        deprecated: false,
        callable: false,
        preferred: false,
        notation: Notation::Identifier,
    }
}
