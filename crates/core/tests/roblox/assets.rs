use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use instar_analysis::Options;

use crate::{
    assets::{fetch, install},
    configuration::Security,
    project::{Change, Project},
    roblox::assets::download,
};

fn files(name: &str) -> io::Result<BTreeMap<String, String>> {
    fs::read_dir(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/assets")
            .join(name),
    )?
    .map(|entry| {
        let entry = entry?;

        Ok((
            entry.file_name().into_string().expect("fixture filename"),
            fs::read_to_string(entry.path())?,
        ))
    })
    .collect()
}

fn server(
    files: BTreeMap<String, String>,
    requests: usize,
    delay: Duration,
) -> io::Result<(String, thread::JoinHandle<io::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}", listener.local_addr()?);

    let worker = thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line)?;

            let name = line
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .trim_start_matches('/')
                .to_owned();

            loop {
                line.clear();
                reader.read_line(&mut line)?;

                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }

            thread::sleep(delay);

            let (status, body) = files
                .get(&name)
                .map_or(("404 Not Found", ""), |body| ("200 OK", body.as_str()));

            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )?;
        }

        Ok(())
    });

    Ok((url, worker))
}

#[test]
fn downloads_verified_profiles_and_loads_cache_without_server() -> io::Result<()> {
    let expected = files("original")?;
    let (url, server) = server(expected.clone(), expected.len(), Duration::ZERO)?;
    let downloaded = fetch(url, download, &Options::new(Duration::from_secs(5)))?;
    server.join().expect("asset server")?;
    assert_eq!(downloaded, expected);

    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("bundle");

    install(
        &cache,
        downloaded,
        &Options::new(Duration::from_secs(5)),
        Instant::now(),
    )?;

    let result = (|| -> io::Result<()> {
        let mut project = Project::new(Duration::from_secs(1));

        for (profile, security) in [
            ("none", Security::None),
            ("local", Security::Local),
            ("plugin", Security::Plugin),
            ("roblox", Security::Roblox),
        ] {
            let assets = project.load_assets(&cache, security)?;

            project.change(Change::Overlay {
                path: directory.path().join("unrelated.luau"),
                text: Some("return 1".to_owned()),
            })?;

            assert!(Rc::ptr_eq(&assets, &project.load_assets(&cache, security)?));
            project.change(Change::Disk(cache.join("metadata.json")))?;

            assert!(!Rc::ptr_eq(
                &assets,
                &project.load_assets(&cache, security)?
            ));

            assert_eq!(
                assets.definitions[0].1.text.as_ref(),
                expected[&format!("{profile}.d.luau")]
            );
        }

        Ok(())
    })();

    directory.close()?;

    result
}

#[test]
fn cold_publishers_preserve_the_first_bundle_and_reader_snapshot() -> io::Result<()> {
    let expected = files("original")?;
    let replacement = files("replacement")?;

    let options = Options::new(Duration::from_secs(5));
    let mut downloaded = Vec::new();

    for bundle in [expected.clone(), replacement] {
        let (url, server) = server(bundle.clone(), bundle.len(), Duration::ZERO)?;
        downloaded.push(fetch(url, download, &options)?);
        server.join().expect("asset server")?;
    }

    let replacement = downloaded.pop().expect("replacement bundle");
    let original = downloaded.pop().expect("original bundle");
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("bundle");
    let metadata = cache.join("metadata.json");
    let (ready, prepared) = mpsc::sync_channel(1);
    let (publish, release) = mpsc::sync_channel(1);

    thread::scope(|scope| -> io::Result<()> {
        let target = cache.clone();
        let manifest = metadata.clone();
        let limits = options.clone();

        let publisher = scope.spawn(move || -> io::Result<()> {
            let mut project = Project::new(Duration::from_secs(1));

            assert_eq!(
                project.source(&manifest).expect_err("cold cache").kind(),
                io::ErrorKind::NotFound
            );

            ready.send(()).expect("publisher ready");

            release
                .recv_timeout(Duration::from_secs(5))
                .expect("release publisher");

            install(&target, replacement, &limits, Instant::now())?;
            project.view.invalidate(&target);
            project.load_assets(&target, Security::None)?;

            Ok(())
        });

        let mut project = Project::new(Duration::from_secs(1));

        assert_eq!(
            project.source(&metadata).expect_err("cold cache").kind(),
            io::ErrorKind::NotFound
        );

        prepared
            .recv_timeout(Duration::from_secs(5))
            .expect("both publishers cold");

        install(&cache, original, &options, Instant::now())?;
        project.view.invalidate(&cache);
        project.source(&metadata)?;
        publish.send(()).expect("release publisher");
        publisher.join().expect("asset publisher")?;

        for (profile, security) in [
            ("none", Security::None),
            ("local", Security::Local),
            ("plugin", Security::Plugin),
            ("roblox", Security::Roblox),
        ] {
            let assets = project.load_assets(&cache, security)?;

            assert_eq!(
                assets.definitions[0].1.text.as_ref(),
                expected[&format!("{profile}.d.luau")]
            );
        }

        for (name, text) in expected {
            assert_eq!(fs::read_to_string(cache.join(name))?, text);
        }

        assert_eq!(fs::read_dir(directory.path())?.count(), 1);

        Ok(())
    })?;

    directory.close()
}

#[test]
fn failed_publication_leaves_no_partial_bundle() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("bundle");
    fs::create_dir_all(cache.join("plugin.d.luau"))?;

    assert!(
        install(
            &cache,
            files("original")?,
            &Options::new(Duration::from_secs(5)),
            Instant::now(),
        )
        .is_err()
    );

    assert_eq!(fs::read_dir(&cache)?.count(), 1);
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);

    directory.close()
}

#[test]
fn rejects_corrupt_downloads_and_http_failures() -> io::Result<()> {
    let mut corrupted = files("original")?;
    corrupted.insert("none.d.luau".to_owned(), "declare forged: any".to_owned());
    let (url, server) = server(corrupted, 2, Duration::ZERO)?;

    assert_eq!(
        fetch(url, download, &Options::new(Duration::from_secs(5)))
            .expect_err("hash mismatch")
            .kind(),
        io::ErrorKind::InvalidData
    );

    server.join().expect("asset server")?;
    let (url, server) = self::server(BTreeMap::new(), 1, Duration::ZERO)?;
    assert!(fetch(url, download, &Options::new(Duration::from_secs(5))).is_err());
    server.join().expect("asset server")?;

    Ok(())
}

#[test]
fn download_obeys_deadline_and_cancellation() -> io::Result<()> {
    let (url, server) = server(files("original")?, 1, Duration::from_millis(300))?;
    let started = Instant::now();

    assert_eq!(
        fetch(url, download, &Options::new(Duration::from_millis(50)))
            .expect_err("deadline")
            .kind(),
        io::ErrorKind::TimedOut
    );

    assert!(started.elapsed() < Duration::from_millis(250));
    drop(server.join().expect("asset server"));
    let options = Options::new(Duration::from_secs(5));
    options.cancellation.cancel();

    assert_eq!(
        fetch("http://127.0.0.1:1".to_owned(), download, &options)
            .expect_err("cancellation")
            .kind(),
        io::ErrorKind::Interrupted
    );

    Ok(())
}
