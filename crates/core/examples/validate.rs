//! Validates a generated Roblox asset bundle against native analysis.

use std::{env, io, path::Path, time::Duration};

use instar_analysis::{Completion, Options};

use instar_core::{
    analysis::Entry,
    project::{Change, Project},
};

fn main() -> io::Result<()> {
    let argument = env::args_os()
        .nth(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected asset directory"))?;

    let directory = std::path::absolute(Path::new(&argument))?;

    for profile in ["none", "local", "plugin", "roblox"] {
        let mut project = Project::new(Duration::from_secs(5));

        let configuration = format!("[roblox]\nenabled=true\nsecurity='{profile}'");
        let cache = instar_core::roblox::cache_directory()?;

        for name in [
            "metadata.json",
            "documentation.json",
            "enumerations.d.luau",
            "none.d.luau",
            "local.d.luau",
            "plugin.d.luau",
            "roblox.d.luau",
        ] {
            project.change(Change::Overlay {
                path: cache.join(name),
                text: Some(std::fs::read_to_string(directory.join(name))?),
            })?;
        }

        project.change(Change::Overlay {
            path: directory.join("instar.toml"),
            text: Some(configuration),
        })?;

        let entry = directory.join("validation.luau");
        project.change(Change::Overlay { path: entry.clone(), text: Some("--!strict\nlocal part: Part = Instance.new('Part')\nlocal service: Players = game:GetService('Players')\nlocal material: Enum.Material = Enum.Material.Plastic\nreturn part, service, material".to_owned()) })?;

        let checked =
            project.check(&[Entry::new(entry)], &Options::new(Duration::from_secs(30)))?;

        if checked.completion != Completion::Complete || !checked.diagnostics.is_empty() {
            return Err(io::Error::other(format!("{profile}: {checked:?}")));
        }
    }

    Ok(())
}
