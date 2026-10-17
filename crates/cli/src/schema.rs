use std::io::{self, Write as _};

use eyre::{Result, WrapErr as _};
use instar_core::settings;

pub(super) fn run() -> Result<u8> {
    let mut output = io::stdout().lock();

    serde_json::to_writer_pretty(&mut output, &settings::schema())
        .wrap_err("cannot write configuration schema")?;

    writeln!(output).wrap_err("cannot finish configuration schema")?;

    Ok(0)
}
