//! Writes the Instar configuration schema to standard output.

use std::{
    error::Error,
    io::{self, Write},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut output = io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, &instar_core::configuration::schema())?;
    writeln!(output)?;

    Ok(())
}
