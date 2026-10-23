use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use instar_bridge::Builtin;
use reqwest::Client;
use serde_json::Value;
use tokio::runtime::Builder;
use tokio_util::sync::CancellationToken;

use super::{Error, publication, reference, rendering};
use crate::settings;

/// A generated configuration or environment asset collection.
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Target {
    /// The configuration JSON Schema.
    Schema,

    /// Luau and Roblox environment assets.
    Environments,
}

impl FromStr for Target {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "schema" => Ok(Self::Schema),
            "environments" => Ok(Self::Environments),

            _ => Err(Error::Invalid {
                reason: format!("Unknown target `{value}`. Expected `schema` or `environments`."),
            }),
        }
    }
}

/// Selected generated files and the number whose contents changed.
pub struct Generated {
    /// Absolute paths of all selected output files.
    pub files: Vec<PathBuf>,

    /// Number of newly created or replaced files.
    pub changed: usize,
}

struct Inputs {
    dump: Value,
    source: String,
    documentation: Value,
    references: reference::Catalog,
}

/// Generate selected configuration and environment assets.
///
/// An empty target list selects every target. Roblox inputs use the tracker's
/// `roblox` branch.
///
/// # Errors
/// Returns download, input, formatting, filesystem, or cancellation failures.
pub fn generate(
    directory: &Path,
    targets: &[Target],
    cancellation: &CancellationToken,
) -> Result<Generated, Error> {
    let is_cancelled = &|| cancellation.is_cancelled();
    interrupted(is_cancelled)?;

    let targets = if targets.is_empty() {
        &[Target::Schema, Target::Environments]
    } else {
        targets
    };

    let mut files = BTreeMap::new();

    if targets.contains(&Target::Schema) {
        let mut source =
            serde_json::to_string_pretty(&settings::schema()).map_err(|source| Error::Data {
                input: "configuration schema".to_owned(),
                source,
            })?;

        source.push('\n');
        files.insert(PathBuf::from("schema.json"), source);
    }

    if targets.contains(&Target::Environments) {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;

        let builtins = instar_bridge::builtins()?;

        let inputs = runtime.block_on(async {
            tokio::select! {
                biased;

                () = cancellation.cancelled() => Err(Error::Cancelled),
                result = acquire(&builtins) => result,
            }
        })?;

        interrupted(is_cancelled)?;

        let rendered = rendering::render(
            inputs.dump,
            &inputs.source,
            inputs.documentation,
            inputs.references,
            &builtins,
            is_cancelled,
        )?;

        files.extend(rendered);
    }

    publication::write(directory, &files, is_cancelled)
}

async fn acquire(builtins: &[Builtin]) -> Result<Inputs, Error> {
    let tracker = "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox";

    let client = Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|source| Error::Download {
            address: tracker.to_owned(),
            source,
        })?;

    let dump = document(&client, &format!("{tracker}/Full-API-Dump.json")).await?;
    let source = download(&client, &format!("{tracker}/LuauTypes.d.luau")).await?;

    let source = String::from_utf8(source).map_err(|error| Error::Invalid {
        reason: format!("Tracker declarations are not valid UTF-8. {error}"),
    })?;

    let documentation = document(&client, &format!("{tracker}/api-docs/en-us.json")).await?;
    let references = reference::acquire(&client, builtins).await?;

    Ok(Inputs {
        dump,
        source,
        documentation,
        references,
    })
}

async fn document(client: &Client, address: &str) -> Result<Value, Error> {
    let contents = download(client, address).await?;

    serde_json::from_slice(&contents).map_err(|source| Error::Data {
        input: address.to_owned(),
        source,
    })
}

pub(super) async fn download(client: &Client, address: &str) -> Result<Vec<u8>, Error> {
    let failure = |source| Error::Download {
        address: address.to_owned(),
        source,
    };

    let response = client.get(address).send().await.map_err(failure)?;
    let response = response.error_for_status().map_err(failure)?;

    Ok(response.bytes().await.map_err(failure)?.to_vec())
}

pub(super) fn interrupted(is_cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
    if is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
