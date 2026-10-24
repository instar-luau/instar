use std::{env, path, time::Instant};

use eyre::{Result, WrapErr as _, bail};

use instar_core::assets::{self, Target};

use super::association;

use crate::{arguments::Assets, interrupt, output};

pub(crate) fn run(options: &Assets) -> Result<u8> {
    let started = Instant::now();

    if options.should_refresh {
        output::phase("Refreshing cached assets", None)?;
        let files = assets::refresh(&interrupt::CANCELLATION)?;
        interrupt::check()?;
        output::refreshed(&files, started);

        return Ok(0);
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

    let root = options
        .root
        .clone()
        .map_or_else(env::current_dir, Ok)
        .wrap_err("Could not resolve the working directory.")?;

    let root = root
        .canonicalize()
        .wrap_err_with(|| format!("Could not resolve project root `{}`.", root.display()))?;

    if !root.is_dir() {
        bail!("Project root `{}` must be a directory.", root.display());
    }

    output::root(&root);

    if !options.should_generate
        && let Some(tool) = options.associate
    {
        interrupt::check()?;
        output::phase("Preparing association", None)?;
        let association = association::prepare(&root, tool, None)?;
        interrupt::check()?;
        output::phase("Finalizing association", None)?;
        association.apply()?;
        interrupt::check()?;
        output::associated(&association.path, &root, started);

        return Ok(0);
    }

    let output = options
        .output
        .as_deref()
        .expect("Internal error: asset generation has no output directory.");

    let output = path::absolute(root.join(output))
        .wrap_err_with(|| format!("Could not resolve output directory `{}`.", output.display()))?;

    interrupt::check()?;
    output::phase("Preparing generation", None)?;

    let association = options
        .associate
        .map(|tool| association::prepare(&root, tool, Some(&output.join("schema.json"))))
        .transpose()?;

    interrupt::check()?;

    let target = if options.targets.len() == 1 {
        Some(match options.targets[0] {
            Target::Schema => "schema",
            Target::Environments => "environments",
        })
    } else {
        None
    };

    output::phase("Generating", target)?;

    let generated = assets::generate(&output, &options.targets, &interrupt::CANCELLATION)?;

    interrupt::check()?;
    output::phase("Finalizing generation", None)?;

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
