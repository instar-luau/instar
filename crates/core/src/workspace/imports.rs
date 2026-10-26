use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Component, Path, PathBuf},
};

use instar_bridge::{Class, Error as AnalysisError, Host as _, HostError, Instance, Navigation};

use crate::syntax::literal::quoted;

use super::{Error, Workspace, environment, host, instances, paths};

impl Workspace {
    /// Generate a require argument for a source, using strings or instance navigation.
    ///
    /// String targets use the requiring module's filesystem or instance context.
    /// Future filesystem paths use lexical navigation beneath their nearest existing ancestor.
    /// Instance targets must identify one `ModuleScript` in the requiring source's hierarchy.
    ///
    /// # Errors
    /// Returns inaccessible paths, invalid configuration, or unavailable sourcemaps.
    pub fn import(
        &self,
        module: &str,
        target: &str,
        is_instance: bool,
    ) -> Result<Option<String>, Error> {
        let module = identity(&self.root.join(module))?;
        let target = identity(&self.root.join(target))?;

        if module == target
            || !paths::is_source(&module)
            || !paths::is_source(&target)
            || paths::metadata(&module)?.is_some_and(|metadata| !metadata.is_file())
            || paths::metadata(&target)?.is_some_and(|metadata| !metadata.is_file())
        {
            return Ok(None);
        }

        let scope = self.scope(&module)?;

        if is_instance || !instances::configuration(self, &scope, &module)?.is_empty() {
            let expression = expression(self, &module, &target, is_instance)?;

            return Ok(if is_instance {
                expression
            } else {
                expression.as_deref().map(quoted)
            });
        }

        let target_scope = self.scope(&target)?;

        if !instances::configuration(self, &target_scope, &target)?.is_empty() {
            return Ok(None);
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

        if let Ok(remainder) = destination.strip_prefix(&source) {
            candidates.insert(format!("@self/{}", paths::text(remainder)?));
        }

        for (name, alias) in &scope.configuration.resolve.aliases {
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

        let should_validate = (paths::metadata(Path::new(&target))?.is_some()
            || self.sources.borrow().contains_key(&target))
            && paths::metadata(parent)?.is_some_and(|metadata| metadata.is_dir());

        let mut candidates: Vec<_> = candidates.into_iter().collect();

        candidates
            .sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));

        for candidate in candidates {
            if !should_validate
                || matches!(resolve(self, &module, &candidate)?, Navigation::Found(found) if found == target)
            {
                return Ok(Some(quoted(&candidate)));
            }
        }

        Ok(None)
    }

    /// Check an auto-import's client/server compatibility.
    ///
    /// # Errors
    /// Returns path, configuration, or sourcemap errors.
    pub fn can_import(&self, module: &str, target: &str) -> Result<bool, Error> {
        let module = identity(&self.root.join(module))?;
        let target = paths::text(&identity(&self.root.join(target))?)?;
        let scope = self.scope(&module)?;
        let locations = instances::configuration(self, &scope, &module)?;
        let registry = self.instances.borrow();

        for location in locations {
            let Some(instances) = registry.get(&location.hierarchy) else {
                continue;
            };

            let source = Runtime::for_instance(instances, location.script);

            for (index, instance) in instances.iter().enumerate() {
                if instance.module.as_deref() == Some(&target)
                    && !source.can_import(Runtime::for_instance(instances, Some(index)))
                {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum Runtime {
    Shared,
    Client,
    Server,
}

impl Runtime {
    fn for_instance(instances: &[Instance], script: Option<usize>) -> Self {
        let Some(mut index) = script else {
            return Self::Shared;
        };

        match instances[index].class.as_str() {
            "LocalScript" => return Self::Client,

            "Script" => {
                // shortcut: missing RunContext metadata, use it when available.
                return if instances[index].module.as_deref().is_some_and(|path| {
                    path.ends_with(".client.lua") || path.ends_with(".client.luau")
                }) {
                    Self::Client
                } else {
                    Self::Server
                };
            }

            _ => {}
        }

        loop {
            match instances[index].class.as_str() {
                "ServerScriptService" | "ServerStorage" => return Self::Server,

                "StarterPlayer"
                | "StarterPlayerScripts"
                | "StarterCharacterScripts"
                | "StarterGui"
                | "StarterPack"
                | "ReplicatedFirst" => return Self::Client,

                _ => {}
            }

            let Some(parent) = instances[index].parent else {
                return Self::Shared;
            };

            index = parent;
        }
    }

    fn can_import(self, target: Self) -> bool {
        self == Self::Shared || target == Self::Shared || self == target
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
                reason: "Expected an absolute source path.",
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

    let mut navigation = if first == "@self" {
        Navigation::Found(module.to_owned())
    } else if let Some(alias) = first.strip_prefix('@') {
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
    is_instance: bool,
) -> Result<Option<String>, Error> {
    let scope = workspace.scope(module)?;
    let locations = instances::configuration(workspace, &scope, module)?;

    if locations.is_empty() {
        return Ok(None);
    }

    let classes = if is_instance {
        environment::identity(workspace, &scope, module)?.and_then(|name| {
            workspace
                .environments
                .borrow()
                .get(&name)
                .and_then(|environment| environment.classes.clone())
        })
    } else {
        None
    };

    let classes = classes.as_deref().unwrap_or_default();
    let target = paths::text(target)?;
    let registry = workspace.instances.borrow();
    let mut candidates: Option<Vec<String>> = None;

    for location in locations {
        let Some(instances) = registry.get(&location.hierarchy) else {
            return Ok(None);
        };

        let relative = navigate(instances, location.script, &target, is_instance, classes);

        let absolute = if is_instance && location.script.is_some() {
            navigate(instances, None, &target, is_instance, classes)
        } else {
            None
        };

        let available = [relative, absolute];

        if let Some(candidates) = &mut candidates {
            candidates
                .retain(|candidate| available.iter().flatten().any(|value| value == candidate));
        } else {
            candidates = Some(available.into_iter().flatten().collect());
        }
    }

    Ok(candidates.and_then(|candidates| candidates.into_iter().next()))
}

fn navigate(
    instances: &[Instance],
    script: Option<usize>,
    target: &str,
    is_instance: bool,
    classes: &[Class],
) -> Option<String> {
    let mut matches = instances.iter().enumerate().filter(|(_, instance)| {
        instance.class == "ModuleScript" && instance.module.as_deref() == Some(target)
    });

    let (destination, _) = matches.next()?;

    if matches.next().is_some() {
        return None;
    }

    let mut ancestors = Vec::new();
    let mut current = script.filter(|_| is_instance);

    while let Some(index) = current {
        if instances[index].class == "DataModel" {
            break;
        }

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
            break match (instances[current].class.as_str(), is_instance) {
                ("DataModel", true) => "game".to_owned(),
                ("DataModel", false) => "@game".to_owned(),
                ("Workspace", true) => "workspace".to_owned(),
                _ => return None,
            };
        };

        children.push(current);
        current = parent;
    };

    for child in children.into_iter().rev() {
        let instance = &instances[child];

        if !is_instance {
            if !is_unique(instances, instance)
                || instance.name.is_empty()
                || matches!(instance.name.as_str(), "." | "..")
                || instance.name.contains(['/', '\\'])
            {
                return None;
            }

            expression.push('/');
            expression.push_str(&instance.name);
            continue;
        }

        let is_service = expression == "game"
            && classes
                .iter()
                .any(|class| class.name == instance.class && class.is_service);

        if is_service {
            if instances
                .iter()
                .filter(|other| other.parent == instance.parent && other.class == instance.class)
                .count()
                != 1
            {
                return None;
            }

            write!(expression, ":GetService({})", quoted(&instance.class))
                .expect("Internal error: writing to a string failed.");
        } else {
            if !is_unique(instances, instance) {
                return None;
            }

            write!(expression, ":WaitForChild({})", quoted(&instance.name))
                .expect("Internal error: writing to a string failed.");
        }
    }

    Some(expression)
}

fn is_unique(instances: &[Instance], target: &Instance) -> bool {
    instances
        .iter()
        .filter(|instance| instance.parent == target.parent && instance.name == target.name)
        .count()
        == 1
}

fn failure(error: HostError) -> Error {
    Error::Analysis(AnalysisError::Host(error))
}
