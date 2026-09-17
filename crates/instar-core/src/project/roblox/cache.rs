use super::{Metadata, invalid};
use crate::analysis::Documentation;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LEVELS: [&str; 4] = ["none", "local", "plugin", "roblox"];
const INTERVAL: u64 = 24 * 60 * 60;
const PAGES: &str = "https://instar-luau.github.io/instar";
const DOCUMENTATION: &str = "https://instar-luau.github.io/instar/documentation.json";

#[derive(Clone)]
pub(super) struct Assets {
    pub source: String,
    pub documentation: Documentation,
}

impl Assets {
    fn validate(&self) -> io::Result<()> {
        metadata(&self.source)?;

        if self.documentation.is_empty() {
            return Err(invalid("empty Roblox documentation"));
        }

        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct Current {
    checked: u64,
}

pub(super) fn metadata(source: &str) -> io::Result<(Metadata, &str)> {
    let (header, definitions) = source
        .split_once('\n')
        .ok_or_else(|| invalid("missing Roblox metadata header"))?;

    if definitions.trim().is_empty() {
        return Err(invalid("empty Roblox definitions"));
    }

    let metadata: Metadata = serde_json::from_str(
        header
            .strip_prefix("--#METADATA#")
            .ok_or_else(|| invalid("invalid Roblox metadata marker"))?,
    )?;

    if metadata
        .classes
        .iter()
        .any(|class| class.name.contains('\0'))
        || metadata.enumerations.iter().any(|name| name.contains('\0'))
    {
        return Err(invalid("Roblox metadata names cannot contain NUL"));
    }

    Ok((metadata, definitions))
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_source(path: &Path) -> io::Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid("cache path has no parent"))?;

    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;

    Ok(())
}

fn write<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_bytes(path, &serde_json::to_vec(value)?)
}

fn source_path(directory: &Path, level: &str) -> io::Result<std::path::PathBuf> {
    if !LEVELS.contains(&level) {
        return Err(invalid(format!("unknown Roblox level {level}")));
    }

    Ok(directory.join(format!("{level}.d.luau")))
}

fn stored(directory: &Path, level: &str) -> io::Result<Option<Assets>> {
    let Some(source) = read_source(&source_path(directory, level)?)? else {
        return Ok(None);
    };

    let Some(documentation) = read::<Documentation>(&directory.join("documentation.json"))? else {
        return Ok(None);
    };

    let assets = Assets {
        source,
        documentation,
    };

    Ok(assets.validate().is_ok().then_some(assets))
}

fn download(level: &str, fetch: &mut impl FnMut(&str) -> io::Result<String>) -> io::Result<Assets> {
    let source = fetch(&format!("{PAGES}/{level}.d.luau"))?;
    metadata(&source)?;

    let documentation: Documentation = serde_json::from_str(&fetch(DOCUMENTATION)?)?;

    let assets = Assets {
        source,
        documentation,
    };

    assets.validate()?;

    Ok(assets)
}

fn cached(
    directory: &Path,
    level: &str,
    force: bool,
    now: u64,
    fetch: &mut impl FnMut(&str) -> io::Result<String>,
) -> io::Result<Assets> {
    let current_path = directory.join("current.json");
    let current = read::<Current>(&current_path)?;
    let existing = stored(directory, level)?;

    if !force
        && current.as_ref().is_some_and(|current| {
            now.checked_sub(current.checked)
                .is_some_and(|elapsed| elapsed < INTERVAL)
        })
        && let Some(assets) = existing.as_ref()
    {
        return Ok(assets.clone());
    }

    match download(level, fetch) {
        Ok(assets) => {
            write_bytes(&source_path(directory, level)?, assets.source.as_bytes())?;
            write(&directory.join("documentation.json"), &assets.documentation)?;
            write(&current_path, &Current { checked: now })?;

            Ok(assets)
        }

        Err(error) if !force => {
            if let (Some(mut current), Some(assets)) = (current, existing) {
                current.checked = now;
                write(&current_path, &current)?;

                eprintln!("roblox: {error}; using cached definitions");

                Ok(assets)
            } else {
                Err(error)
            }
        }

        Err(error) => Err(error),
    }
}

pub(super) fn load(directory: &Path, level: &str, force: bool) -> io::Result<Assets> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();

    cached(directory, level, force, now, &mut |url| {
        let agent = ureq::AgentBuilder::new()
            .https_only(true)
            .tls_connector(std::sync::Arc::new(
                ureq::native_tls::TlsConnector::new().map_err(io::Error::other)?,
            ))
            .timeout(Duration::from_secs(60))
            .build();

        let limit = 512 * 1024 * 1024;
        let mut bytes = Vec::new();

        agent
            .get(url)
            .call()
            .map_err(io::Error::other)?
            .into_reader()
            .take(limit + 1)
            .read_to_end(&mut bytes)?;

        if bytes.len() as u64 > limit {
            return Err(invalid("Roblox download exceeds the size limit"));
        }

        String::from_utf8(bytes).map_err(|error| invalid(error.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(kind: &str) -> String {
        format!("--#METADATA#{{\"classes\":[],\"enumerations\":[]}}\ndeclare sample: {kind}")
    }

    fn fetch(url: &str) -> io::Result<String> {
        if url == DOCUMENTATION {
            Ok(r#"{"sample":"description"}"#.into())
        } else if LEVELS
            .into_iter()
            .any(|level| url == format!("{PAGES}/{level}.d.luau"))
        {
            Ok(source("number"))
        } else {
            Err(io::Error::other("unexpected request"))
        }
    }

    fn offline(_: &str) -> io::Result<String> {
        Err(io::Error::other("offline"))
    }

    #[test]
    fn downloads_checks_daily_and_forces_refresh() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let level = "plugin";
        let source_path = format!("{PAGES}/{level}.d.luau");
        let mut requests = Vec::new();

        cached(root, level, false, 1, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests, [source_path.clone(), DOCUMENTATION.to_owned()]);

        cached(root, level, false, INTERVAL, &mut |_| {
            panic!("fresh cache requested the network")
        })?;

        requests.clear();

        cached(root, level, false, INTERVAL + 1, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests, [source_path.clone(), DOCUMENTATION.to_owned()]);
        requests.clear();

        cached(root, level, true, INTERVAL + 2, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests, [source_path.clone(), DOCUMENTATION.to_owned()]);
        assert_eq!(fs::read_dir(root)?.count(), 3);

        let updated = cached(root, level, false, INTERVAL * 2 + 2, &mut |url| {
            if url == source_path {
                Ok(source("string"))
            } else {
                fetch(url)
            }
        })?;

        assert!(updated.source.ends_with("declare sample: string"));

        assert_eq!(
            read::<Current>(&root.join("current.json"))?
                .unwrap()
                .checked,
            INTERVAL * 2 + 2
        );

        Ok(())
    }

    #[test]
    fn failed_updates_keep_the_previous_assets_and_back_off() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let level = "plugin";
        cached(root, level, false, 1, &mut fetch)?;
        let path = root.join("plugin.d.luau");
        let original = fs::read(&path)?;

        cached(root, level, false, INTERVAL + 1, &mut offline)?;

        assert_eq!(fs::read(&path)?, original);

        cached(root, level, false, INTERVAL + 2, &mut |_| {
            panic!("failed check was immediately retried")
        })?;

        let current = fs::read(root.join("current.json"))?;
        assert!(cached(root, level, true, INTERVAL + 3, &mut offline).is_err());
        assert_eq!(fs::read(root.join("current.json"))?, current);

        Ok(())
    }

    #[test]
    fn corrupt_cache_and_clock_rollback_require_refresh() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let level = "plugin";
        cached(root, level, false, 10, &mut fetch)?;
        let mut requests = Vec::new();

        cached(root, level, false, 1, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests.len(), 2);
        fs::write(root.join("plugin.d.luau"), "invalid")?;
        requests.clear();

        cached(root, level, false, 2, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests.len(), 2);

        fs::write(root.join("current.json"), r#"{"checked":"invalid"}"#)?;
        cached(root, level, false, 3, &mut fetch)?;

        Ok(())
    }

    #[test]
    fn rejects_incomplete_assets_and_invalid_metadata() -> io::Result<()> {
        let assets = Assets {
            source: source("number"),
            documentation: [("sample".to_owned(), serde_json::json!("description"))]
                .into_iter()
                .collect(),
        };

        assets.validate()?;

        assert!(
            Assets {
                source: "declare sample: number".into(),
                ..assets.clone()
            }
            .validate()
            .is_err()
        );

        assert!(metadata("declare sample: number").is_err());
        assert!(metadata("--#METADATA#{}\n").is_err());
        assert!(metadata("--#METADATA#{\"classes\":[],\"enumerations\":[\"sample\\u0000\"]}\ndeclare sample: number").is_err());

        Ok(())
    }
}
