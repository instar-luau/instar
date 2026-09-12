use super::{Metadata, invalid};
use crate::{analysis::Documentation, configuration::RobloxConfig};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LEVELS: [&str; 4] = ["none", "local", "plugin", "roblox"];
const INTERVAL: u64 = 24 * 60 * 60;
const LATEST: &str =
    "https://api.github.com/repos/instar-luau/instar/commits?path=generated/bundle.json";

#[derive(Deserialize, Serialize)]
pub(super) struct Bundle {
    pub definitions: BTreeMap<String, String>,
    pub documentation: Documentation,
}

impl Bundle {
    fn validate(&self) -> io::Result<()> {
        for level in LEVELS {
            let source = self
                .definitions
                .get(level)
                .ok_or_else(|| invalid(format!("missing Roblox level {level}")))?;

            metadata(source)?;
        }

        if self.documentation.is_empty() {
            return Err(invalid("empty Roblox documentation"));
        }

        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct Current {
    revision: String,
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

fn revision(value: &str) -> io::Result<&str> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid(
            "Roblox revision must be a full 40-character commit hash",
        ));
    }

    Ok(value)
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid("cache path has no parent"))?;

    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;

    Ok(())
}

fn stored(directory: &Path, selected: &str) -> io::Result<Option<Bundle>> {
    Ok(
        read::<Bundle>(&directory.join(format!("{}.json", revision(selected)?)))?
            .filter(|bundle| bundle.validate().is_ok()),
    )
}

fn download(
    selected: &str,
    fetch: &mut impl FnMut(&str) -> io::Result<String>,
) -> io::Result<Bundle> {
    let url = format!(
        "https://raw.githubusercontent.com/instar-luau/instar/{}/generated/bundle.json",
        revision(selected)?
    );

    let bundle: Bundle = serde_json::from_str(&fetch(&url)?)?;
    bundle.validate()?;

    Ok(bundle)
}

fn cached(
    directory: &Path,
    pinned: Option<&str>,
    force: bool,
    now: u64,
    fetch: &mut impl FnMut(&str) -> io::Result<String>,
) -> io::Result<Bundle> {
    if let Some(selected) = pinned {
        revision(selected)?;

        if !force && let Some(bundle) = stored(directory, selected)? {
            return Ok(bundle);
        }

        let bundle = download(selected, fetch)?;
        write(&directory.join(format!("{selected}.json")), &bundle)?;

        return Ok(bundle);
    }

    let path = directory.join("current.json");
    let current = read::<Current>(&path)?.filter(|current| revision(&current.revision).is_ok());

    let existing = current
        .as_ref()
        .map(|current| stored(directory, &current.revision))
        .transpose()?
        .flatten();

    if !force
        && current.as_ref().is_some_and(|current| {
            now.checked_sub(current.checked)
                .is_some_and(|elapsed| elapsed < INTERVAL)
        })
        && let Some(bundle) = existing
    {
        return Ok(bundle);
    }

    let update = (|| {
        #[derive(Deserialize)]
        struct Commit {
            sha: String,
        }

        let commits: Vec<Commit> = serde_json::from_str(&fetch(LATEST)?)?;

        let selected = &commits
            .first()
            .ok_or_else(|| invalid("no published Roblox bundles"))?
            .sha;

        revision(selected)?;

        let bundle = if !force
            && current
                .as_ref()
                .is_some_and(|current| current.revision == *selected)
            && existing.is_some()
        {
            None
        } else {
            Some(download(selected, fetch)?)
        };

        Ok::<_, io::Error>((selected.clone(), bundle))
    })();

    match update {
        Ok((selected, bundle)) => {
            let bundle = match bundle {
                Some(bundle) => {
                    write(&directory.join(format!("{selected}.json")), &bundle)?;

                    bundle
                }

                None => existing.ok_or_else(|| invalid("missing cached Roblox bundle"))?,
            };

            write(
                &path,
                &Current {
                    revision: selected,
                    checked: now,
                },
            )?;

            Ok(bundle)
        }

        Err(error) if !force => {
            if let (Some(mut current), Some(bundle)) = (current, existing) {
                current.checked = now;
                write(&path, &current)?;

                eprintln!(
                    "roblox: {error}; using cached revision {}",
                    current.revision
                );

                Ok(bundle)
            } else {
                Err(error)
            }
        }

        Err(error) => Err(error),
    }
}

pub(super) fn load(configuration: &RobloxConfig, force: bool) -> io::Result<Bundle> {
    let directory = configuration
        .cache
        .clone()
        .or_else(|| dirs::cache_dir().map(|path| path.join("instar").join("roblox")))
        .ok_or_else(|| io::Error::other("cannot locate the Roblox cache directory"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();

    cached(
        &directory,
        configuration.revision.as_deref(),
        force,
        now,
        &mut |url| {
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
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "1111111111111111111111111111111111111111";
    const SECOND: &str = "2222222222222222222222222222222222222222";

    fn source() -> String {
        let definitions: BTreeMap<_, _> = LEVELS
            .into_iter()
            .map(|level| {
                (
                    level,
                    "--#METADATA#{\"classes\":[],\"enumerations\":[]}\ndeclare sample: number",
                )
            })
            .collect();

        serde_json::json!({"definitions": definitions, "documentation": {"sample": "description"}})
            .to_string()
    }

    fn fetch(url: &str) -> io::Result<String> {
        if url == LATEST {
            Ok(serde_json::json!([{ "sha": FIRST }]).to_string())
        } else if url.ends_with("/bundle.json") {
            Ok(source())
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
        let mut requests = Vec::new();

        cached(root, None, false, 1, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains(FIRST));

        cached(root, None, false, INTERVAL, &mut |_| {
            panic!("fresh cache requested the network")
        })?;

        requests.clear();

        cached(root, None, false, INTERVAL + 1, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests, [LATEST]);
        requests.clear();

        cached(root, None, true, INTERVAL + 2, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests.len(), 2);
        assert_eq!(fs::read_dir(root)?.count(), 2);

        let updated = cached(root, None, false, INTERVAL * 2 + 2, &mut |url| {
            if url == LATEST {
                Ok(serde_json::json!([{ "sha": SECOND }]).to_string())
            } else {
                Ok(source().replace("number", "string"))
            }
        })?;

        assert!(updated.definitions["plugin"].ends_with("declare sample: string"));

        assert_eq!(
            read::<Current>(&root.join("current.json"))?
                .unwrap()
                .revision,
            SECOND
        );

        assert!(
            stored(root, FIRST)?.unwrap().definitions["plugin"].ends_with("declare sample: number")
        );

        Ok(())
    }

    #[test]
    fn pins_are_offline_and_failures_preserve_cached_assets() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();

        cached(root, Some(FIRST), false, 1, &mut |url| {
            assert_ne!(url, LATEST);
            assert!(url.contains(FIRST));

            fetch(url)
        })?;

        assert!(!root.join("current.json").exists());
        let path = root.join(format!("{FIRST}.json"));
        let original = fs::read(&path)?;

        cached(root, Some(FIRST), false, u64::MAX, &mut |_| {
            panic!("pin requested the network")
        })?;

        assert!(cached(root, Some(FIRST), true, 1, &mut offline).is_err());
        assert!(cached(root, Some(SECOND), false, 1, &mut offline).is_err());
        assert_eq!(fs::read(&path)?, original);

        assert!(
            cached(root, Some("../outside"), false, 1, &mut |_| panic!(
                "invalid revision requested the network"
            ))
            .is_err()
        );

        Ok(())
    }

    #[test]
    fn failed_updates_keep_the_previous_revision_and_back_off() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        assert!(cached(root, None, false, 1, &mut offline).is_err());
        cached(root, None, false, 1, &mut fetch)?;
        let path = root.join(format!("{FIRST}.json"));
        let original = fs::read(&path)?;

        cached(root, None, false, INTERVAL + 1, &mut |url| {
            if url == LATEST {
                Ok(serde_json::json!([{ "sha": SECOND }]).to_string())
            } else {
                Ok("{}".into())
            }
        })?;

        assert_eq!(fs::read(&path)?, original);
        assert!(!root.join(format!("{SECOND}.json")).exists());

        cached(root, None, false, INTERVAL + 2, &mut |_| {
            panic!("failed check was immediately retried")
        })?;

        let current = fs::read(root.join("current.json"))?;
        assert!(cached(root, None, true, INTERVAL + 3, &mut offline).is_err());
        assert_eq!(fs::read(root.join("current.json"))?, current);

        Ok(())
    }

    #[test]
    fn corrupt_cache_and_clock_rollback_require_refresh() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        cached(root, None, false, 10, &mut fetch)?;
        let mut requests = Vec::new();

        cached(root, None, false, 1, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests, [LATEST]);
        fs::write(root.join(format!("{FIRST}.json")), "invalid")?;
        requests.clear();

        cached(root, None, false, 2, &mut |url| {
            requests.push(url.to_owned());

            fetch(url)
        })?;

        assert_eq!(requests.len(), 2);

        fs::write(
            root.join("current.json"),
            r#"{"revision":"../outside","checked":2}"#,
        )?;

        cached(root, None, false, 3, &mut fetch)?;

        Ok(())
    }

    #[test]
    fn cache_paths_and_revisions_inherit_and_override() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let child = root.join("child");
        fs::create_dir(&child)?;

        fs::write(
            root.join("instar.toml"),
            format!("[roblox]\ncache = 'shared'\nrevision = '{FIRST}'"),
        )?;

        fs::write(child.join("instar.toml"), "[roblox]")?;

        let inherited =
            crate::project::resolution::Resolver::new(&mut crate::source::SourceStore::default())
                .discovery
                .roblox(&child.join("main.luau"))?
                .unwrap();

        assert_eq!(inherited.cache, Some(root.join("shared")));
        assert_eq!(inherited.revision.as_deref(), Some(FIRST));

        fs::write(
            child.join("instar.toml"),
            format!("[roblox]\ncache = 'local'\nrevision = '{SECOND}'"),
        )?;

        let overridden =
            crate::project::resolution::Resolver::new(&mut crate::source::SourceStore::default())
                .discovery
                .roblox(&child.join("main.luau"))?
                .unwrap();

        assert_eq!(overridden.cache, Some(child.join("local")));
        assert_eq!(overridden.revision.as_deref(), Some(SECOND));

        Ok(())
    }

    #[test]
    fn rejects_incomplete_assets_and_invalid_metadata() -> io::Result<()> {
        let mut bundle: Bundle = serde_json::from_str(&source())?;
        bundle.validate()?;
        bundle.definitions.remove("none");
        assert!(bundle.validate().is_err());
        assert!(metadata("declare sample: number").is_err());
        assert!(metadata("--#METADATA#{}\n").is_err());
        assert!(metadata("--#METADATA#{\"classes\":[],\"enumerations\":[\"sample\\u0000\"]}\ndeclare sample: number").is_err());

        Ok(())
    }
}
