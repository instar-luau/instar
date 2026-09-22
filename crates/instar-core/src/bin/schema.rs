//! Write the generated project configuration schema to a requested path.

use std::{env, error::Error, fs, path::PathBuf};

use instar_core::config::Config;
use schemars::schema_for;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("schema output path is required")?);

    if args.next().is_some() {
        return Err("expected one schema output path".into());
    }

    let schema = serde_json::to_string_pretty(&schema_for!(Config))?;

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, format!("{schema}\n"))?;

    Ok(())
}
