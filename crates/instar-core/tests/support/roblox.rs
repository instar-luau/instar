use std::{fs, io, path::Path};

pub(super) fn configure(root: &Path) -> io::Result<()> {
    let directory = root.join("cache");
    fs::create_dir_all(&directory)?;
    let revision = "0000000000000000000000000000000000000000";
    let snapshot = directory.join(format!("{revision}.json"));

    if !snapshot.exists() {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../generated/bundle.json"),
            snapshot,
        )?;
    }

    let configuration = root.join("instar.toml");
    let contents = fs::read_to_string(&configuration)?;

    if !contents.contains("\ncache = 'cache'\n") {
        fs::write(
            configuration,
            format!("{contents}\ncache = 'cache'\nrevision = '{revision}'\n"),
        )?;
    }

    Ok(())
}
