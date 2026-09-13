use super::{Dependency, Graft, Manifest, cache, requirement};
use crate::configuration::InstarConfig;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Cursor, Read},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const DOWNLOAD_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

fn select<'release>(
    releases: &'release [Release],
    requested: &str,
) -> io::Result<(Version, &'release Release)> {
    let exact = Version::parse(requested).ok();
    let requested = requirement(requested)?;

    releases
        .iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = Version::parse(
                release
                    .tag_name
                    .strip_prefix('v')
                    .unwrap_or(&release.tag_name),
            )
            .ok()?;

            (requested.matches(&version)
                && exact.as_ref().is_none_or(|exact| exact == &version)
                && (!release.prerelease || !version.pre.is_empty()))
            .then_some((version, release))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .ok_or_else(|| io::Error::other("no compatible graft release"))
}

fn platform() -> Option<String> {
    if !matches!(std::env::consts::ARCH, "x86_64" | "aarch64") {
        return None;
    }

    let suffix = match std::env::consts::OS {
        "windows" if cfg!(target_env = "msvc") => "pc-windows-msvc",
        "windows" if cfg!(target_env = "gnu") => "pc-windows-gnu",
        "linux" if cfg!(target_env = "gnu") => "unknown-linux-gnu",
        "linux" if cfg!(target_env = "musl") => "unknown-linux-musl",
        "macos" => "apple-darwin",
        _ => return None,
    };

    Some(format!("{}-{suffix}", std::env::consts::ARCH))
}

fn asset<'release>(
    release: &'release Release,
    name: &str,
    platform: Option<&str>,
) -> io::Result<&'release Asset> {
    let names = platform
        .map(|platform| format!("{name}-{platform}.zip"))
        .into_iter()
        .chain(std::iter::once(format!("{name}.zip")));

    for name in names {
        let mut matches = release.assets.iter().filter(|asset| asset.name == name);

        if let Some(asset) = matches.next() {
            if matches.next().is_some() {
                return Err(io::Error::other("ambiguous graft release assets"));
            }

            return Ok(asset);
        }
    }

    Err(io::Error::other(
        "graft release has no platform or portable project ZIP",
    ))
}

fn bytes(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(DOWNLOAD_LIMIT + 1).read_to_end(&mut bytes)?;

    if bytes.len() as u64 > DOWNLOAD_LIMIT {
        return Err(io::Error::other("graft download exceeds the size limit"));
    }

    Ok(bytes)
}

fn releases(agent: &ureq::Agent, repository: &str) -> io::Result<Vec<Release>> {
    let endpoint = format!("https://api.github.com/repos/{repository}/releases");
    let mut next = Some(endpoint.clone());
    let mut visited = std::collections::BTreeSet::new();
    let mut releases = Vec::new();

    while let Some(address) = next.take() {
        if !(address == endpoint || address.starts_with(&format!("{endpoint}?")))
            || !visited.insert(address.clone())
        {
            return Err(io::Error::other("invalid GitHub release pagination"));
        }

        let response = agent.get(&address).call().map_err(io::Error::other)?;

        next = response.header("Link").and_then(|header| {
            header.split(',').find_map(|link| {
                let (address, relation) = link.trim().split_once(';')?;

                (relation.trim() == "rel=\"next\"")
                    .then(|| {
                        address
                            .trim()
                            .strip_prefix('<')?
                            .strip_suffix('>')
                            .map(str::to_owned)
                    })
                    .flatten()
            })
        });

        let page: Vec<Release> =
            serde_json::from_slice(&bytes(response.into_reader())?).map_err(io::Error::other)?;

        releases.extend(page);
    }

    Ok(releases)
}

fn verify(bytes: &[u8], digest: Option<&str>) -> io::Result<()> {
    if let Some(digest) = digest {
        let expected = digest
            .strip_prefix("sha256:")
            .ok_or_else(|| io::Error::other("unsupported graft asset digest"))?;

        let actual = Sha256::digest(bytes);

        if expected.len() != actual.len() * 2
            || actual.iter().enumerate().any(|(index, byte)| {
                expected
                    .get(index * 2..index * 2 + 2)
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                    != Some(*byte)
            })
        {
            return Err(io::Error::other("graft asset SHA-256 digest mismatch"));
        }
    }

    Ok(())
}

fn extract(bytes: &[u8], destination: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(io::Error::other)?;
    let mut total = 0;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(io::Error::other)?;

        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| io::Error::other("unsafe graft ZIP path"))?;

        if entry.name().starts_with('/')
            || entry.name().contains(['\\', ':'])
            || enclosed.as_os_str().is_empty()
            || enclosed
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || entry
                .name()
                .split('/')
                .any(|part| part == ".." || part == "." || part.ends_with(['.', ' ']))
        {
            return Err(io::Error::other("unsafe graft ZIP path"));
        }

        for part in enclosed.components() {
            let name = part.as_os_str().to_string_lossy();

            let stem = name
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();

            if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
            {
                return Err(io::Error::other("unsafe graft ZIP filename"));
            }
        }

        let mode = entry.unix_mode().unwrap_or(0);

        if !matches!(mode & 0o170_000, 0 | 0o100_000 | 0o040_000) {
            return Err(io::Error::other(
                "graft ZIP links and special files are unsupported",
            ));
        }

        let path = destination.join(enclosed);

        if entry.is_dir() {
            fs::create_dir_all(path)?;
            continue;
        }

        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("ZIP entry has no parent"))?,
        )?;

        let mut file = fs::File::create_new(&path)?;

        let count = io::copy(
            &mut (&mut entry).take(DOWNLOAD_LIMIT - total + 1),
            &mut file,
        )?;

        total += count;

        if total > DOWNLOAD_LIMIT {
            return Err(io::Error::other(
                "expanded graft project exceeds the size limit",
            ));
        }

        #[cfg(unix)]
        if mode != 0 {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
        }
    }

    Ok(())
}

fn validate(directory: &Path, name: &str, version: &Version) -> io::Result<()> {
    let path = directory.join("instar.toml");
    let manifest = Manifest::read(&path)?;

    if manifest.name != name
        || Version::parse(&manifest.version).map_err(io::Error::other)? != *version
    {
        return Err(io::Error::other(
            "graft project identity or version does not match its release",
        ));
    }

    Graft::load(&path, name)?;

    Ok(())
}

fn project(
    cache: &Path,
    repository: &str,
    name: &str,
    version: &Version,
    asset: &Asset,
    download: impl FnOnce(&str) -> io::Result<Vec<u8>>,
) -> io::Result<PathBuf> {
    let parent = cache.join(repository).join(name);
    let destination = parent.join(version.to_string());

    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_dir() => {
            validate(&destination, name, version)?;

            return Ok(destination);
        }

        Ok(_) => {
            return Err(io::Error::other(
                "graft cache destination is not a directory",
            ));
        }

        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let bytes = download(&asset.browser_download_url)?;
    verify(&bytes, asset.digest.as_deref())?;
    fs::create_dir_all(&parent)?;
    let temporary = tempfile::tempdir_in(&parent)?;
    extract(&bytes, temporary.path())?;
    validate(temporary.path(), name, version)?;
    fs::rename(temporary.path(), &destination)?;

    Ok(destination)
}

/// Install configured graft projects, fetching remote releases only in this operation.
///
/// # Errors
/// Returns configuration, release selection, download, validation, or cache publication errors.
pub fn install(path: &Path) -> io::Result<Vec<PathBuf>> {
    let path = crate::source::absolute(path).map_err(io::Error::other)?;
    let metadata = fs::metadata(&path)?;

    if !metadata.is_dir()
        && (!metadata.is_file() || path.file_name().is_none_or(|name| name != "instar.toml"))
    {
        return Err(io::Error::other(
            "expected a project directory or instar.toml",
        ));
    }

    let directory = if metadata.is_dir() {
        path.as_path()
    } else {
        path.parent()
            .ok_or_else(|| io::Error::other("configuration has no parent"))?
    };

    let mut dependencies = BTreeMap::new();
    let mut found = false;

    for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let configuration = ancestor.join("instar.toml");

        let text = match fs::read_to_string(&configuration) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };

        found = true;
        let parsed = InstarConfig::parse(&text).map_err(io::Error::other)?;

        for (name, dependency) in parsed.grafts.unwrap_or_default() {
            dependencies.insert(name, (dependency, ancestor.to_owned()));
        }
    }

    if !found {
        return Err(io::Error::other("cannot find instar.toml"));
    }

    let mut installed = Vec::new();

    for (name, (dependency, directory)) in dependencies {
        match &dependency {
            Dependency::Local { .. } => {
                let path = dependency.resolve(&directory, &name)?;
                Graft::load(&path, &name)?;

                installed.push(
                    path.parent()
                        .ok_or_else(|| io::Error::other("graft has no parent"))?
                        .to_owned(),
                );
            }

            Dependency::Remote { repo, version, .. } => {
                let agent = ureq::AgentBuilder::new()
                    .https_only(true)
                    .tls_connector(Arc::new(
                        ureq::native_tls::TlsConnector::new().map_err(io::Error::other)?,
                    ))
                    .timeout(Duration::from_secs(60))
                    .build();

                let releases = releases(&agent, repo)?;
                let (version, release) = select(&releases, version)?;
                let asset = asset(release, &name, platform().as_deref())?;

                installed.push(project(
                    &cache()?,
                    repo,
                    &name,
                    &version,
                    asset,
                    |address| {
                        bytes(
                            agent
                                .get(address)
                                .call()
                                .map_err(io::Error::other)?
                                .into_reader(),
                        )
                    },
                )?);
            }
        }
    }

    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    const METADATA: &str = "[graft]\nname='example'\nversion='0.2.1'\nprotocol=1\nruntime='luau'\nentry='dist/module.luau'\nlint=true\n";
    const MODULE: &str = "return table.freeze({lint=function() return {} end})";

    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));

        for (name, text) in files {
            archive
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();

            archive.write_all(text.as_bytes()).unwrap();
        }

        archive.finish().unwrap().into_inner()
    }

    fn release(tag: &str, assets: &[&str]) -> Release {
        Release {
            tag_name: tag.to_owned(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|name| Asset {
                    name: (*name).to_owned(),
                    browser_download_url: "fixture".to_owned(),
                    digest: None,
                })
                .collect(),
        }
    }

    #[test]
    fn versions_and_assets_select_exact_compatible_and_platform_projects() {
        let mut releases = vec![
            release("v0.2.0", &["example.zip"]),
            release(
                "0.2.1",
                &["example.zip", "example-x86_64-pc-windows-msvc.zip"],
            ),
            release("v0.3.0", &["example.zip"]),
            release("unrelated", &[]),
            release("v0.2.9", &[]),
            release("v0.2.2-preview.1", &[]),
        ];

        let variants = [release("v0.2.1+first", &[]), release("v0.2.1+second", &[])];

        assert_eq!(
            select(&variants, "0.2.1+first").unwrap().0.build.as_str(),
            "first"
        );

        assert!(select(&variants, "0.2.1").is_err());
        releases[4].draft = true;
        releases[5].prerelease = true;
        assert_eq!(select(&releases, "0.2.0").unwrap().0, Version::new(0, 2, 0));

        assert_eq!(
            select(&releases, "=0.2.0").unwrap().0,
            Version::new(0, 2, 0)
        );

        let (version, selected) = select(&releases, "^0.2.0").unwrap();
        assert_eq!(version, Version::new(0, 2, 1));

        assert_eq!(
            select(&releases, "0.2.2-preview.1").unwrap().0.pre.as_str(),
            "preview.1"
        );

        assert!(select(&releases, "0.2.5").is_err());

        assert_eq!(
            asset(selected, "example", Some("x86_64-pc-windows-msvc"))
                .unwrap()
                .name,
            "example-x86_64-pc-windows-msvc.zip"
        );

        assert_eq!(
            asset(selected, "example", Some("aarch64-apple-darwin"))
                .unwrap()
                .name,
            "example.zip"
        );

        assert_eq!(
            asset(selected, "example", None).unwrap().name,
            "example.zip"
        );

        assert!(asset(selected, "another", None).is_err());

        assert!(
            asset(
                &release("v0.2.1", &["example.zip", "example.zip"]),
                "example",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn digests_accept_verified_or_absent_hashes_and_reject_invalid_downloads() {
        let digest = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(b"abc", Some(digest)).is_ok());
        assert!(verify(b"abc", None).is_ok());
        assert!(verify(b"changed", Some(digest)).is_err());

        for digest in ["sha256:", "sha256:invalid", "sha512:abc", ""] {
            assert!(verify(b"abc", Some(digest)).is_err());
        }

        assert_eq!(bytes(Cursor::new(b"abc")).unwrap(), b"abc");
    }

    #[test]
    fn extraction_rejects_traversal_absolute_paths_links_and_collisions() {
        for name in [
            "../escape",
            "/escape",
            "nested/../../escape",
            "nested/../escape",
            "C:/escape",
            "C:escape",
            "nested\\escape",
            "NUL",
            "CON.txt",
            "trailing.",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let destination = directory.path().join("project");
            fs::create_dir(&destination).unwrap();

            assert!(
                extract(&archive(&[(name, "bad")]), &destination).is_err(),
                "{name}"
            );

            assert!(!directory.path().join("escape").exists());
        }

        let directory = tempfile::tempdir().unwrap();
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));

        archive
            .add_symlink("link", "../escape", SimpleFileOptions::default())
            .unwrap();

        let archive = archive.finish().unwrap().into_inner();
        assert!(extract(&archive, directory.path()).is_err());
        let contents = self::archive(&[("file", "first")]);
        extract(&contents, directory.path()).unwrap();
        assert!(extract(&contents, directory.path()).is_err());

        assert_eq!(
            fs::read_to_string(directory.path().join("file")).unwrap(),
            "first"
        );
    }

    #[test]
    fn projects_validate_before_publication_and_reuse_complete_cached_projects() {
        let directory = tempfile::tempdir().unwrap();
        let release = release("v0.2.1", &["example.zip"]);
        let asset = &release.assets[0];
        let version = Version::new(0, 2, 1);

        let contents = archive(&[
            ("instar.toml", METADATA),
            ("dist/module.luau", MODULE),
            ("source/helper.luau", "return 1"),
        ]);

        let installed = project(
            directory.path(),
            "owner/project",
            "example",
            &version,
            asset,
            |_| Ok(contents),
        )
        .unwrap();

        assert_eq!(
            installed,
            directory.path().join("owner/project/example/0.2.1")
        );

        assert_eq!(
            fs::read_to_string(installed.join("source/helper.luau")).unwrap(),
            "return 1"
        );

        assert_eq!(
            project(
                directory.path(),
                "owner/project",
                "example",
                &version,
                asset,
                |_| panic!("cache must be reused")
            )
            .unwrap(),
            installed
        );

        let dependency = Dependency::Remote {
            repo: "owner/project".to_owned(),
            version: "^0.2.0".to_owned(),
            configuration: BTreeMap::new(),
        };

        let path = dependency.cached(directory.path(), "example").unwrap();

        assert!(
            Graft::load(&path, "example")
                .unwrap()
                .lint(b"return 1")
                .unwrap()
                .is_empty()
        );

        assert!(
            dependency
                .cached(directory.path(), "missing")
                .unwrap_err()
                .to_string()
                .contains("run instar graft install")
        );

        fs::write(
            installed.join("instar.toml"),
            METADATA.replace("0.2.1", "0.2.2"),
        )
        .unwrap();

        assert!(dependency.cached(directory.path(), "example").is_err());

        assert!(
            project(
                directory.path(),
                "owner/project",
                "example",
                &version,
                asset,
                |_| panic!("invalid cache must not be overwritten")
            )
            .is_err()
        );
    }

    #[test]
    fn rejected_projects_never_become_cached_versions() {
        let release = release("v0.2.1", &["example.zip"]);
        let version = Version::new(0, 2, 1);

        for (metadata, module) in [
            (METADATA.replace("name='example'", "name='another'"), MODULE),
            (METADATA.replace("0.2.1", "0.2.2"), MODULE),
            (METADATA.replace("protocol=1", "protocol=2"), MODULE),
            (
                METADATA.replace("dist/module.luau", "../module.luau"),
                MODULE,
            ),
            (METADATA.replace("dist/module.luau", "missing.luau"), MODULE),
            (METADATA.replace("lint=true", "lint=false"), MODULE),
            (METADATA.replace("[graft]", "[unknown]"), MODULE),
            (METADATA.to_owned(), "not luau"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let contents = archive(&[("instar.toml", &metadata), ("dist/module.luau", module)]);

            assert!(
                project(
                    directory.path(),
                    "owner/project",
                    "example",
                    &version,
                    &release.assets[0],
                    |_| Ok(contents)
                )
                .is_err(),
                "{metadata}"
            );

            assert!(
                !directory
                    .path()
                    .join("owner/project/example/0.2.1")
                    .exists()
            );

            assert_eq!(
                fs::read_dir(directory.path().join("owner/project/example"))
                    .unwrap()
                    .count(),
                0
            );
        }

        let directory = tempfile::tempdir().unwrap();
        let mut asset = release.assets.into_iter().next().unwrap();
        asset.digest = Some("sha256:invalid".to_owned());

        assert!(
            project(
                directory.path(),
                "owner/project",
                "example",
                &version,
                &asset,
                |_| Ok(Vec::new())
            )
            .is_err()
        );

        assert!(!directory.path().join("owner").exists());

        assert!(
            project(
                directory.path(),
                "owner/project",
                "example",
                &version,
                &asset,
                |_| Err(io::Error::other("download failed"))
            )
            .is_err()
        );

        assert!(!directory.path().join("owner").exists());
    }
}
