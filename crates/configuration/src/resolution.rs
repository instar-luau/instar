//! Module alias and sourcemap configuration.

use std::{collections::BTreeMap, mem, path::PathBuf};

use figment::{
    Error,
    value::{Dict, Value},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::schema;

/// Module resolution inputs.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Explicit sourcemaps; an empty list disables automatic discovery.
    #[schemars(extend("items" = schema::path()))]
    pub sourcemaps: Option<Vec<PathBuf>>,

    /// Case-insensitive alias names mapped to paths relative to their defining file.
    #[schemars(extend("propertyNames" = schema::alias(), "additionalProperties" = schema::path()))]
    pub aliases: BTreeMap<String, PathBuf>,
}

pub(super) fn normalize(values: &mut Dict) -> Result<(), Box<Error>> {
    let Some(Value::Dict(_, resolution)) = values.get_mut("resolve") else {
        return Ok(());
    };

    let Some(Value::Dict(_, aliases)) = resolution.get_mut("aliases") else {
        return Ok(());
    };

    for (name, value) in mem::take(aliases) {
        let canonical = name.to_ascii_lowercase();

        if aliases.insert(canonical, value).is_some() {
            return Err(Box::new(
                Error::from("duplicate case-insensitive alias")
                    .with_path("resolve")
                    .with_path("aliases")
                    .with_path(&name),
            ));
        }
    }

    Ok(())
}
