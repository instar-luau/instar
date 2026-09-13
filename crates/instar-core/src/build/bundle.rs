use super::{
    configuration::Target,
    graph::{self, Module},
    mapping::Text,
    syntax::quote,
    transform,
};

use crate::roblox::Environment;

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

pub(super) fn emit(
    modules: &BTreeMap<PathBuf, Module>,
    root: &Path,
    entry: &Path,
    environment: &Environment,
    target: Target,
) -> io::Result<Text> {
    let mut prefix = String::from("__instar_");

    while modules
        .values()
        .any(|module| module.text.text.contains(&prefix))
    {
        prefix.push('_');
    }

    let mut output = Text::default();

    output.generated(&format!(
        "local {prefix}entry = {}\n",
        quote(
            &entry
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/")
        )
    ));

    output.generated(&include_str!("runtime.luau").replace("__instar_", &prefix));

    for (path, module) in modules {
        let identifier = path
            .strip_prefix(root)
            .map_err(io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");

        output.generated(&format!(
            "\n{prefix}modules[{}] = function()\n",
            quote(&identifier)
        ));

        let text = graph::rewrite(
            module,
            &BTreeMap::new(),
            environment,
            target,
            Some((&prefix, root)),
        )?;

        let text = transform::exports(&text)?;
        output.append(&text);
        output.generated("\nend\n");
    }

    output.generated(&format!(
        "\nreturn {prefix}require({})\n",
        quote(
            &entry
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/")
        )
    ));

    transform::validate(&output.text)?;

    Ok(output)
}
