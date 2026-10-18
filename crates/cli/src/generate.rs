use std::{
    path::{self, Path},
    time::Instant,
};

use eyre::{Result, WrapErr as _, bail};
use instar_core::assets::{self, Target};

use super::{
    arguments::{Arguments, Generate},
    association, interrupt, report,
};

pub(super) fn run(arguments: &Arguments, options: &Generate) -> Result<u8> {
    let started = Instant::now();

    if arguments.stdin_filepath.is_some() {
        bail!("--stdin-filepath cannot be used with generate");
    }

    if options.associate.is_some()
        && !options.targets.is_empty()
        && !options.targets.contains(&Target::Schema)
    {
        bail!("--associate requires the schema target");
    }

    if options.revision.is_some()
        && !options.targets.is_empty()
        && options
            .targets
            .iter()
            .all(|target| *target == Target::Schema)
    {
        bail!("--revision requires the luau or roblox target");
    }

    let root = options.root.as_deref().unwrap_or_else(|| Path::new("."));

    let root = path::absolute(root)
        .wrap_err_with(|| format!("cannot resolve root `{}`", root.display()))?;

    if !root.is_dir() {
        bail!("project root `{}` must be a directory", root.display());
    }

    let output = path::absolute(root.join(&options.output))
        .wrap_err_with(|| format!("cannot resolve output `{}`", options.output.display()))?;

    interrupt::check()?;

    let association = options
        .associate
        .map(|tool| association::prepare(&root, tool, &output.join("schema.json")))
        .transpose()?;

    interrupt::check()?;

    let generated = assets::generate(
        &output,
        &options.targets,
        options.revision.as_deref(),
        interrupt::requested,
    )?;

    interrupt::check()?;

    if let Some(configuration) = &association {
        configuration.apply()?;
    }

    interrupt::check()?;

    report::generated(
        arguments,
        &generated,
        &root,
        association
            .as_ref()
            .map(|configuration| configuration.path.as_path()),
        started,
    );

    Ok(0)
}
