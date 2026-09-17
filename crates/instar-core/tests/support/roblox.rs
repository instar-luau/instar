use std::{fs, io, path::Path};

pub(super) fn configure(root: &Path) -> io::Result<()> {
    let directory = root.join(".instar/roblox");
    fs::create_dir_all(&directory)?;

    let generated: serde_json::Value = serde_json::from_slice(&fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../generated/bundle.json"),
    )?)?;

    for level in ["none", "local", "plugin", "roblox"] {
        let source = generated["definitions"][level].as_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "missing Roblox definitions")
        })?;

        fs::write(directory.join(format!("{level}.d.luau")), source)?;
    }

    fs::write(
        directory.join("documentation.json"),
        serde_json::to_vec(&generated["documentation"])?,
    )?;

    fs::write(
        directory.join("current.json"),
        serde_json::to_vec(&serde_json::json!({
            "checked": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_secs(),
        }))?,
    )?;

    Ok(())
}
