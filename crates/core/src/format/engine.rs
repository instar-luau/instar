use vermis::tree::Tree;

use crate::{
    format::{
        Error, Prepared,
        directives::{self, Directives, Kind},
        imports::Imports,
        layout, preservation,
    },
    settings::format::Order,
    syntax,
};

/// Format a complete UTF-8 Luau source using the supplied configuration.
///
/// Preserves an encoding marker and formatting-disabled regions.
///
/// # Errors
/// Returns malformed syntax or directives, or a failed preservation check.
pub fn format(source: &str, configuration: &Prepared) -> Result<String, Error> {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let result = formatted(&source[prefix..], configuration);

    match result {
        Ok(output) => Ok(format!("{}{output}", &source[..prefix])),

        Err(mut error) => {
            error.range.start += prefix;
            error.range.end += prefix;

            Err(error)
        }
    }
}

pub(crate) fn parse(source: &[u8]) -> Result<Tree<'_>, Error> {
    syntax::parse(source, |range, message| {
        Error::input(range, message.unwrap_or("incomplete syntax"))
    })
}

fn formatted(source: &str, configuration: &Prepared) -> Result<String, Error> {
    let tree = parse(source.as_bytes())?;
    let directives = directives::collect(&tree)?;

    if directives.policies.iter().any(|directive| {
        matches!(directive.kind, Kind::Verbatim | Kind::Remainder)
            && directive.scope == (0..source.len())
    }) {
        return Ok(source.to_owned());
    }

    let imports =
        (configuration.settings().require.order != Order::Preserve).then(|| Imports::new(&tree));

    render(&tree, configuration, imports.as_ref(), &directives)
}

pub(crate) fn render(
    tree: &Tree<'_>,
    configuration: &Prepared,
    imports: Option<&Imports>,
    directives: &Directives,
) -> Result<String, Error> {
    let plan = layout::Builder::build(tree, configuration, imports, directives)?;
    let bytes = layout::emit(tree, configuration.settings(), &plan)?;

    let output = String::from_utf8(bytes).map_err(|error| {
        Error::internal(0..0, format!("formatter produced invalid UTF-8: {error}"))
    })?;

    validate(tree, &output, configuration, imports, directives)?;

    Ok(output)
}

pub(crate) fn validate(
    tree: &Tree<'_>,
    output: &str,
    configuration: &Prepared,
    imports: Option<&Imports>,
    directives: &Directives,
) -> Result<(), Error> {
    let proposed = parse(output.as_bytes()).map_err(|error| {
        Error::internal(0..0, format!("formatter produced invalid syntax: {error}"))
    })?;

    let proposed_directives = directives::collect(&proposed).map_err(|error| {
        Error::internal(
            0..0,
            format!("formatter produced invalid directives: {error}"),
        )
    })?;

    preservation::validate(
        tree,
        &proposed,
        configuration.settings(),
        imports,
        directives,
        &proposed_directives,
    )?;

    Ok(())
}
