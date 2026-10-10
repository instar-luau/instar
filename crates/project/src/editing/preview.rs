use std::{fmt::Write, io, time::Duration};

use instar_shared::Options;
use similar::{ChangeTag, TextDiff};

pub(crate) fn preview(
    original: &str,
    proposed: &str,
    name: &str,
    options: &Options,
) -> io::Result<String> {
    options.check()?;
    let mut configuration = TextDiff::configure();

    configuration.timeout(
        options
            .remaining()
            .unwrap_or(Duration::from_millis(50))
            .min(Duration::from_millis(50)),
    );

    let diff = configuration.diff_lines(original, proposed);
    options.check()?;
    let mut output = format!("--- {name}\n+++ {name}\n");

    for hunk in diff.unified_diff().iter_hunks() {
        options.check()?;
        writeln!(&mut output, "{}", hunk.header()).map_err(io::Error::other)?;

        for change in hunk.iter_changes() {
            options.check()?;

            let sign = match change.tag() {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            };

            write!(&mut output, "{sign}{}", change.value()).map_err(io::Error::other)?;

            if change.missing_newline() {
                output.push_str("\n\\ No newline at end of file\n");
            }

            if output.len() > 2 * 1024 * 1024 {
                return Err(instar_shared::Reason::AnalysisLimit.error());
            }
        }
    }

    Ok(output)
}
