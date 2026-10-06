use super::*;

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
};

fn files() -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut profiles = BTreeMap::new();
    let mut properties = BTreeMap::new();

    for profile in PROFILES {
        let text = format!("declare {profile}: string");
        profiles.insert(profile, format!("{:x}", Sha256::digest(text.as_bytes())));
        properties.insert(profile, Vec::<String>::new());
        files.insert(format!("{profile}.d.luau"), text);
    }

    files.insert("enumerations.d.luau".to_owned(), String::new());
    files.insert("documentation.json".to_owned(), "{}".to_owned());

    files.insert(
        "metadata.json".to_owned(),
        serde_json::json!({
            "revision": "0123456789012345678901234567890123456789",
            "profiles": profiles,
            "properties": properties,
            "services": [],
            "creatable_instances": [],
            "enumerations": format!("{:x}", Sha256::digest(b"")),
            "documentation": format!("{:x}", Sha256::digest(b"{}")),
        })
        .to_string(),
    );

    files
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
    let expected = files();
    let (url, server) = server(expected.clone(), expected.len(), Duration::ZERO)?;
    let downloaded = fetch(url, &Options::new(Duration::from_secs(5)))?;
    server.join().expect("asset server")?;
    assert_eq!(downloaded, expected);

    let directory = std::env::temp_dir().join(format!(
        "instar-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    install(&directory, downloaded)?;

    let result = (|| -> io::Result<()> {
        let mut project = Project::new(Duration::from_secs(1));

        for (profile, security) in [
            ("none", Security::None),
            ("local", Security::Local),
            ("plugin", Security::Plugin),
            ("roblox", Security::Roblox),
        ] {
            let assets = project.load_assets(&directory, security)?;

            assert_eq!(
                assets.definitions[0].1.text.as_ref(),
                expected[&format!("{profile}.d.luau")]
            );
        }

        Ok(())
    })();

    fs::remove_dir_all(directory)?;

    result
}

#[test]
fn rejects_corrupt_downloads_and_http_failures() -> io::Result<()> {
    let mut corrupted = files();
    corrupted.insert("none.d.luau".to_owned(), "declare forged: any".to_owned());
    let (url, server) = server(corrupted, 2, Duration::ZERO)?;

    assert_eq!(
        fetch(url, &Options::new(Duration::from_secs(5)))
            .expect_err("hash mismatch")
            .kind(),
        io::ErrorKind::InvalidData
    );

    server.join().expect("asset server")?;
    let (url, server) = self::server(BTreeMap::new(), 1, Duration::ZERO)?;
    assert!(fetch(url, &Options::new(Duration::from_secs(5))).is_err());
    server.join().expect("asset server")?;

    Ok(())
}

#[test]
fn download_obeys_deadline_and_cancellation() -> io::Result<()> {
    let (url, server) = server(files(), 1, Duration::from_millis(300))?;
    let started = Instant::now();

    assert_eq!(
        fetch(url, &Options::new(Duration::from_millis(50)))
            .expect_err("deadline")
            .kind(),
        io::ErrorKind::TimedOut
    );

    assert!(started.elapsed() < Duration::from_millis(250));
    drop(server.join().expect("asset server"));
    let options = Options::new(Duration::from_secs(5));
    options.cancellation.cancel();

    assert_eq!(
        fetch("http://127.0.0.1:1".to_owned(), &options)
            .expect_err("cancellation")
            .kind(),
        io::ErrorKind::Interrupted
    );

    Ok(())
}
