use std::io::{self, Write};

use similar::TextDiff;

pub(super) fn render(
    writer: &mut dyn Write,
    name: &str,
    original: &str,
    output: &str,
) -> io::Result<()> {
    let difference = TextDiff::from_lines(original, output);

    let patch = difference
        .unified_diff()
        .context_radius(3)
        .header(name, name)
        .to_string();

    for line in patch.lines().take(400) {
        writeln!(writer, "{line}")?;
    }

    if patch.lines().count() > 400 {
        writeln!(writer, "... patch truncated after 400 lines")?;
    }

    Ok(())
}
