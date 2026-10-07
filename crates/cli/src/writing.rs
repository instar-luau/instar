use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, Write},
    path::Path,
};

use eyre::{Result, WrapErr, ensure};

pub(crate) fn replace(path: &Path, original: &[u8], output: &[u8]) -> Result<()> {
    if original == output {
        return Ok(());
    }

    let metadata = fs::symlink_metadata(path)?;

    ensure!(
        metadata.is_file(),
        "refusing to replace a nonregular file: {}",
        path.display()
    );

    let mut file = OpenOptions::new().read(true).write(true).open(path)?;

    file.try_lock()
        .wrap_err_with(|| format!("file is locked: {}", path.display()))?;

    let mut current = Vec::new();
    file.read_to_end(&mut current)?;

    ensure!(
        current == original,
        "file changed during formatting: {}",
        path.display()
    );

    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().expect("absolute source directory"))?;

    temporary.write_all(output)?;

    temporary
        .as_file()
        .set_permissions(metadata.permissions())?;

    temporary.as_file().sync_all()?;
    file.rewind()?;
    current.clear();
    file.read_to_end(&mut current)?;

    ensure!(
        current == original && fs::metadata(path)?.modified()? == metadata.modified()?,
        "file changed during formatting: {}",
        path.display()
    );

    drop(file);

    temporary
        .persist(path)
        .wrap_err_with(|| format!("replacing {}", path.display()))?;

    Ok(())
}
