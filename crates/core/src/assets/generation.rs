use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use reqwest::Client;
use serde_json::Value;
use tokio::{runtime::Builder, time};

use super::{Error, publication::Publication, rendering};
use crate::settings;

const REPOSITORY: &str = "MaximumADHD/Roblox-Client-Tracker";

/// A generated configuration or environment asset collection.
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Target {
    /// The configuration JSON Schema.
    Schema,

    /// Luau standard-library documentation.
    Luau,

    /// Roblox declarations, metadata, and documentation.
    Roblox,
}

impl Target {
    pub(super) fn files(self) -> &'static [&'static str] {
        match self {
            Self::Schema => &["schema.json"],
            Self::Luau => &["luau/metadata.json", "luau/documentation.json"],

            Self::Roblox => &[
                "roblox/metadata.json",
                "roblox/documentation.json",
                "roblox/none.d.luau",
                "roblox/local.d.luau",
                "roblox/plugin.d.luau",
                "roblox/roblox.d.luau",
            ],
        }
    }
}

impl FromStr for Target {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "schema" => Ok(Self::Schema),
            "luau" => Ok(Self::Luau),
            "roblox" => Ok(Self::Roblox),

            _ => Err(Error::Invalid {
                reason: format!("unknown target `{value}`; expected schema, luau, or roblox"),
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
}

/// Generate selected configuration and environment assets.
///
/// An empty target list selects every target. Upstream inputs use the current
/// tracker revision unless a full commit identity is supplied.
///
/// # Errors
/// Returns download, input, formatting, filesystem, or cancellation failures.
pub fn generate(
    directory: &Path,
    targets: &[Target],
    revision: Option<&str>,
    cancelled: fn() -> bool,
) -> Result<Generated, Error> {
    interrupted(cancelled)?;

    let targets = if targets.is_empty() {
        &[Target::Schema, Target::Luau, Target::Roblox]
    } else {
        targets
    };

    let upstream = targets.iter().any(|target| *target != Target::Schema);

    if let Some(revision) = revision {
        if !upstream {
            return Err(Error::Invalid {
                reason: "revision requires a luau or roblox target".to_owned(),
            });
        }

        validate_revision(revision)?;
    }

    let publication = Publication::new(directory, targets)?;
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

    if upstream {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;

        let inputs = runtime.block_on(async {
            tokio::select! {
                biased;

                () = async {
                    while !cancelled() {
                        time::sleep(Duration::from_millis(50)).await;
                    }
                } => Err(Error::Cancelled),
                result = acquire(revision) => result,
            }
        })?;

        interrupted(cancelled)?;
        let builtins = instar_luau::declarations()?;

        let mut rendered = rendering::render(
            inputs.dump,
            &inputs.source,
            inputs.documentation,
            &builtins,
            cancelled,
        )?;

        rendered.retain(|path, _| publication.contains(path));
        files.extend(rendered);
    }

    publication.write(&files, cancelled)
}

async fn acquire(revision: Option<&str>) -> Result<Inputs, Error> {
    let address = format!("https://api.github.com/repos/{REPOSITORY}/commits/roblox");

    let client = Client::builder()
        .https_only(true)
        .user_agent(concat!("instar/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|source| Error::Download {
            address: address.clone(),
            source,
        })?;

    let revision = match revision {
        Some(revision) => revision.to_owned(),

        None => document(&client, &address)
            .await?
            .get("sha")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid {
                reason: "missing tracker revision".to_owned(),
            })?
            .to_owned(),
    };

    validate_revision(&revision)?;

    let tracker = format!("https://raw.githubusercontent.com/{REPOSITORY}/{revision}");
    let dump = document(&client, &format!("{tracker}/Full-API-Dump.json")).await?;
    let source = download(&client, &format!("{tracker}/LuauTypes.d.luau")).await?;

    let source = String::from_utf8(source).map_err(|error| Error::Invalid {
        reason: format!("invalid UTF-8 in tracker declarations: {error}"),
    })?;

    let documentation = document(&client, &format!("{tracker}/api-docs/en-us.json")).await?;

    Ok(Inputs {
        dump,
        source,
        documentation,
    })
}

async fn document(client: &Client, address: &str) -> Result<Value, Error> {
    let contents = download(client, address).await?;

    serde_json::from_slice(&contents).map_err(|source| Error::Data {
        input: address.to_owned(),
        source,
    })
}

async fn download(client: &Client, address: &str) -> Result<Vec<u8>, Error> {
    let failure = |source| Error::Download {
        address: address.to_owned(),
        source,
    };

    let response = client.get(address).send().await.map_err(failure)?;
    let response = response.error_for_status().map_err(failure)?;

    Ok(response.bytes().await.map_err(failure)?.to_vec())
}

fn validate_revision(revision: &str) -> Result<(), Error> {
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(Error::Invalid {
            reason: "invalid tracker revision".to_owned(),
        });
    }

    Ok(())
}

pub(super) fn interrupted(cancelled: fn() -> bool) -> Result<(), Error> {
    if cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
