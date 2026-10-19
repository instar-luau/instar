use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Component, Path, PathBuf},
};

use instar_bridge::{Error as AnalysisError, Host as _, HostError, Instance, Navigation};

use super::{Error, Workspace, host, instances, paths};

impl Workspace {
    /// Generate a require argument for a source, using strings or instance navigation.
    ///
    /// Existing string targets must resolve unambiguously through workspace navigation.
    /// Future paths use lexical navigation beneath their nearest existing ancestor.
    /// Instance targets must identify one `ModuleScript` in the requiring source's hierarchy.
    ///
    /// # Errors
    /// Returns inaccessible paths, invalid configuration, or unavailable sourcemaps.
    pub fn import(
        &self,
        module: &str,
        target: &str,
        instance: bool,
    ) -> Result<Option<String>, Error> {
        let module = identity(&self.root.join(module))?;
        let target = identity(&self.root.join(target))?;

        if module == target
            || !paths::source(&module)
            || !paths::source(&target)
            || paths::metadata(&module)?.is_some_and(|metadata| !metadata.is_file())
            || paths::metadata(&target)?.is_some_and(|metadata| !metadata.is_file())
        {
            return Ok(None);
        }

        if instance {
            return expression(self, &module, &target);
        }

        let source = host::logical(&module)?;
        let destination = host::logical(&target)?;

        let Some(parent) = source.parent() else {
            return Ok(None);
        };

        let mut candidates = BTreeSet::new();

        if let Some(relative) = relative(parent, &destination)? {
            candidates.insert(if relative.starts_with("../") {
                relative
            } else {
                format!("./{relative}")
            });
        }

        for (name, alias) in &self.scope(&module)?.configuration.resolve.aliases {
            if name == "self" {
                continue;
            }

            let alias = host::logical(&identity(alias)?)?;

            if let Ok(remainder) = destination.strip_prefix(&alias) {
                let remainder = paths::text(remainder)?;

                candidates.insert(if remainder.is_empty() {
                    format!("@{name}")
                } else {
                    format!("@{name}/{remainder}")
                });
            }
        }

        let target = paths::text(&target)?;
        let module = paths::text(&module)?;

        let validate = (paths::metadata(Path::new(&target))?.is_some()
            || self.sources.borrow().contains_key(&target))
            && paths::metadata(parent)?.is_some_and(|metadata| metadata.is_dir());

        let mut candidates: Vec<_> = candidates.into_iter().collect();

        candidates
            .sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));

        for candidate in candidates {
            if !validate
                || matches!(resolve(self, &module, &candidate)?, Navigation::Found(found) if found == target)
            {
                return Ok(Some(quoted(&candidate)));
            }
        }

        Ok(None)
    }
}

fn identity(path: &Path) -> Result<PathBuf, Error> {
    if paths::metadata(path)?.is_some() {
        return paths::canonical(path);
    }

    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            component => normalized.push(component.as_os_str()),
        }
    }

    let mut ancestor = normalized.as_path();
    let mut remainder = Vec::new();

    while paths::metadata(ancestor)?.is_none() {
        let (Some(parent), Some(name)) = (ancestor.parent(), ancestor.file_name()) else {
            return Err(Error::Path {
                path: path.to_owned(),
                reason: "expected an absolute source path",
            });
        };

        remainder.push(name);
        ancestor = parent;
    }

    let mut path = if remainder.is_empty() {
        paths::canonical(ancestor)?
    } else {
        paths::directory(ancestor)?
    };

    for component in remainder.into_iter().rev() {
        path.push(component);
    }

    Ok(path)
}

fn relative(base: &Path, target: &Path) -> Result<Option<String>, Error> {
    let base: Vec<_> = base.components().collect();
    let target: Vec<_> = target.components().collect();

    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();

    if common == 0 {
        return Ok(None);
    }

    let mut path = PathBuf::new();

    for _ in common..base.len() {
        path.push("..");
    }

    for component in &target[common..] {
        path.push(component.as_os_str());
    }

    paths::text(&path).map(Some)
}

fn resolve(workspace: &Workspace, module: &str, path: &str) -> Result<Navigation, Error> {
    let mut components = path.split('/');
    let first = components.next().unwrap_or_default();

    let mut navigation = if let Some(alias) = first.strip_prefix('@') {
        workspace.alias(module, alias).map_err(failure)?
    } else {
        let parent = workspace.parent(module).map_err(failure)?;

        if first == ".." {
            match parent {
                Navigation::Found(parent) => workspace.parent(&parent).map_err(failure)?,
                navigation => navigation,
            }
        } else {
            parent
        }
    };

    for component in components {
        let Navigation::Found(current) = navigation else {
            return Ok(navigation);
        };

        navigation = match component {
            "" | "." => Navigation::Found(current),
            ".." => workspace.parent(&current).map_err(failure)?,
            component => workspace.child(&current, component).map_err(failure)?,
        };
    }

    Ok(navigation)
}

fn expression(
    workspace: &Workspace,
    module: &Path,
    target: &Path,
) -> Result<Option<String>, Error> {
    let scope = workspace.scope(module)?;
    let locations = instances::configuration(workspace, &scope, module)?;

    let Some(first) = locations.first() else {
        return Ok(None);
    };

    if locations
        .iter()
        .any(|location| location.hierarchy != first.hierarchy)
    {
        return Ok(None);
    }

    let Some(instances) = workspace.instances.borrow().get(&first.hierarchy) else {
        return Ok(None);
    };

    let target = paths::text(target)?;

    let mut matches = instances.iter().enumerate().filter(|(_, instance)| {
        instance.class == "ModuleScript" && instance.module.as_deref() == Some(target.as_str())
    });

    let Some((destination, _)) = matches.next() else {
        return Ok(None);
    };

    if matches.next().is_some() {
        return Ok(None);
    }

    let script = (locations.len() == 1).then_some(first.script).flatten();
    let mut ancestors = Vec::new();
    let mut current = script;

    while let Some(index) = current {
        ancestors.push(index);
        current = instances[index].parent;
    }

    let mut children = Vec::new();
    let mut current = destination;

    let mut expression = loop {
        if let Some(distance) = ancestors.iter().position(|ancestor| *ancestor == current) {
            break format!("script{}", ".Parent".repeat(distance));
        }

        let Some(parent) = instances[current].parent else {
            break match instances[current].class.as_str() {
                "DataModel" => "game".to_owned(),
                "Workspace" => "workspace".to_owned(),
                _ => return Ok(None),
            };
        };

        children.push(current);
        current = parent;
    };

    for child in children.into_iter().rev() {
        let instance = &instances[child];

        if !unique(&instances, instance) {
            return Ok(None);
        }

        write!(expression, ":WaitForChild({})", quoted(&instance.name))
            .expect("internal error: writing to a string failed");
    }

    Ok(Some(expression))
}

fn unique(instances: &[Instance], target: &Instance) -> bool {
    instances
        .iter()
        .filter(|instance| instance.parent == target.parent && instance.name == target.name)
        .count()
        == 1
}

fn quoted(text: &str) -> String {
    let mut quoted = String::from("\"");

    for character in text.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),

            character if character.is_ascii_control() => {
                write!(quoted, "\\{:03}", u32::from(character))
                    .expect("internal error: writing to a string failed");
            }

            character => quoted.push(character),
        }
    }

    quoted.push('"');

    quoted
}

fn failure(error: HostError) -> Error {
    Error::Analysis(AnalysisError::Host(error))
}
