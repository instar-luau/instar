use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use figment::Error;
use globset::escape;

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
            for (index, pattern) in patterns.iter_mut().enumerate() {
                let field = field.field(&index.to_string());

                if !Path::new(pattern).is_absolute() {
                    let directory = field
                        .base(base)
                        .to_str()
                        .ok_or_else(|| field.error("glob base directories must be UTF-8"))?;

                    let directory = escape(&directory.replace(MAIN_SEPARATOR, "/"));
                    let anchored = Path::new(&directory).join(&*pattern);

                    *pattern = anchored
                        .to_str()
                        .ok_or_else(|| field.error("glob patterns must be UTF-8"))?
                        .replace(MAIN_SEPARATOR, "/");
                }
            }
        }
    }

    let resolution = source.field("resolve");

    for (name, target) in &mut configuration.resolve.aliases {
        anchor(target, &resolution.field("aliases").field(name), base);
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

    let environment = source.field("environment");

    for (name, path) in [
        (
            "asset_directory",
            &mut configuration.environment.asset_directory,
        ),
        ("capabilities", &mut configuration.environment.capabilities),
    ] {
        if let Some(path) = path {
            anchor(path, &environment.field(name), base);
        }
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
