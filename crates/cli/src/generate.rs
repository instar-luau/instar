use std::{
    path::{self, Path},
    time::Instant,
};

use eyre::{Result, WrapErr as _, bail};
use instar_core::assets::{self, Target};

use super::{
    arguments::{Arguments, Generate},
    association, interrupt, output,
};

pub(super) fn run(arguments: &Arguments, options: &Generate) -> Result<u8> {
    let started = Instant::now();

    if arguments.stdin_filepath.is_some() {
        return Err(output::Validation::new(
            "--stdin-filepath cannot be used with generate.",
            "Remove --stdin-filepath.",
        )
        .into());
    }

    if options.associate.is_some()
        && !options.targets.is_empty()
        && !options.targets.contains(&Target::Schema)
    {
        return Err(output::Validation::new(
            "--associate requires the schema target.",
            "Include schema in the selected targets, or remove --associate.",
        )
        .into());
    }

    if options.revision.is_some()
        && !options.targets.is_empty()
        && options
            .targets
            .iter()
            .all(|target| *target == Target::Schema)
    {
        return Err(output::Validation::new(
            "--revision requires the luau or roblox target.",
            "Include luau or roblox in the selected targets, or remove --revision.",
        )
        .into());
    }

    let root = options.root.as_deref().unwrap_or_else(|| Path::new("."));

    let root = path::absolute(root)
        .wrap_err_with(|| format!("Could not resolve project root `{}`.", root.display()))?;

    if !root.is_dir() {
        bail!("Project root `{}` must be a directory.", root.display());
    }

    output::root(&root);

    let output = path::absolute(root.join(&options.output)).wrap_err_with(|| {
        format!(
            "Could not resolve output directory `{}`.",
            options.output.display()
        )
    })?;

    interrupt::check()?;
    output::generation("Preparing generation", None)?;

    let association = options
        .associate
        .map(|tool| association::prepare(&root, tool, &output.join("schema.json")))
        .transpose()?;

    interrupt::check()?;

    let target = if options.targets.len() == 1 {
        Some(match options.targets[0] {
            Target::Schema => "schema",
            Target::Luau => "luau",
            Target::Roblox => "roblox",
        })
    } else {
        None
    };

    output::generation("Generating", target)?;

    let generated = assets::generate(
        &output,
        &options.targets,
        options.revision.as_deref(),
        interrupt::requested,
    )?;

    interrupt::check()?;
    output::generation("Finalizing generation", None)?;

    if let Some(configuration) = &association {
        configuration.apply()?;
    }

    interrupt::check()?;

    output::generated(
        &generated,
        &root,
        association
            .as_ref()
            .map(|configuration| configuration.path.as_path()),
        started,
    );

    Ok(0)
}
