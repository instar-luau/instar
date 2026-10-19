use cxx::Exception;

use crate::bridge::bindings;

include!(concat!(env!("OUT_DIR"), "/builtins.rs"));

pub(crate) fn declarations(
    settings: &[bindings::Flag],
    scoped: &[bindings::Flag],
) -> Result<String, Exception> {
    let selectors = bindings::selectors(settings, scoped, &SELECTORS)?;
    let mut source = assemble(&selectors);

    source.push_str(
        "\ndeclare string: any\ndeclare next: any\ndeclare pairs: any\ndeclare getmetatable: any\ndeclare setmetatable: any\n",
    );

    Ok(source)
}
