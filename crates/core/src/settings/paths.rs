use std::path::{Path, PathBuf};

use figment::Error;

use super::{Configuration, source::Source};

pub(super) fn resolve(
    configuration: &mut Configuration,
    source: &Source<'_>,
    base: &Path,
) -> Result<(), Box<Error>> {
    for (field, patterns) in [
        (source.field("include"), &mut configuration.include),
        (source.field("exclude"), &mut configuration.exclude),
        (
            source.field("check").field("include"),
            &mut configuration.check.include,
        ),
        (
            source.field("check").field("exclude"),
            &mut configuration.check.exclude,
        ),
        (
            source.field("index").field("include"),
            &mut configuration.index.include,
        ),
        (
            source.field("index").field("exclude"),
            &mut configuration.index.exclude,
        ),
        (
            source.field("format").field("include"),
            &mut configuration.format.include,
        ),
        (
            source.field("format").field("exclude"),
            &mut configuration.format.exclude,
        ),
        (
            source.field("lint").field("include"),
            &mut configuration.lint.include,
        ),
        (
            source.field("lint").field("exclude"),
            &mut configuration.lint.exclude,
        ),
    ] {
        if let Some(patterns) = patterns {
            patterns.resolve(&field, base)?;
        }
    }

    let resolution = source.field("resolve");

    for (name, target) in &mut configuration.resolve.aliases {
        let field = resolution.field("aliases").field(name);

        if let Ok(relative) = target.strip_prefix("~") {
            let home = dirs::home_dir()
                .ok_or_else(|| field.error("Could not locate the home directory."))?;

            *target = home.join(relative);
        }

        anchor(target, &field, base);
    }

    if let Some(sourcemaps) = &mut configuration.resolve.sourcemaps {
        for (index, path) in sourcemaps.iter_mut().enumerate() {
            anchor(
                path,
                &resolution.field("sourcemaps").field(&index.to_string()),
                base,
            );
        }
    }

    if let Some(generator) = &mut configuration.resolve.generator {
        let source = resolution.field("generator");
        let command = source.field("command");

        if generator
            .command
            .first()
            .is_some_and(|executable| executable.trim().is_empty())
        {
            return Err(command.field("0").error("Expected a nonempty executable."));
        }

        for (name, arguments) in [("command", &generator.command), ("watch", &generator.watch)] {
            for (index, argument) in arguments.iter().enumerate() {
                if argument.contains('\0') {
                    return Err(source
                        .field(name)
                        .field(&index.to_string())
                        .error("Command arguments must not contain NUL characters."));
                }
            }
        }

        command.base(base).clone_into(&mut generator.directory);
    }

    let environment = source.field("environment");

    if let Some(path) = &mut configuration.environment.capabilities {
        anchor(path, &environment.field("capabilities"), base);
    }

    for (index, group) in configuration.environment.groups.iter_mut().enumerate() {
        let field = environment.field("groups").field(&index.to_string());

        for (name, paths) in [
            ("definitions", &mut group.definitions),
            ("documentation", &mut group.documentation),
        ] {
            for (index, path) in paths.iter_mut().enumerate() {
                anchor(path, &field.field(name).field(&index.to_string()), base);
            }
        }
    }

    Ok(())
}

fn anchor(path: &mut PathBuf, source: &Source<'_>, base: &Path) {
    *path = source.base(base).join(&*path);
}
