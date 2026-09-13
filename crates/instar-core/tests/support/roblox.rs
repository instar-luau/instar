use std::{fs, io, path::Path};

pub(super) fn configure(root: &Path) -> io::Result<()> {
    let directory = root.join(".instar/roblox");
    fs::create_dir_all(&directory)?;
    let revision = "0000000000000000000000000000000000000000";
    let snapshot = directory.join(format!("{revision}.json"));

    if !snapshot.exists() {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../generated/bundle.json"),
            snapshot,
        )?;
    }

    fs::write(
        directory.join("current.json"),
        serde_json::to_vec(&serde_json::json!({
            "revision": revision,
            "checked": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_secs(),
        }))?,
    )?;

    Ok(())
}
