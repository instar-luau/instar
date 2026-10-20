//! Watch-mode event filtering and incremental checking.

use std::{
    error::Error,
    fs,
    io::{self, BufRead as _, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const EXECUTABLE: &str = env!("CARGO_BIN_EXE_instar");

struct Session {
    child: Child,
    output: Receiver<io::Result<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Session {
    fn start(root: &Path) -> Result<Self, Box<dyn Error>> {
        fs::write(
            root.join("custom.toml"),
            "exclude = [\"ignored/**\"]\n[environment]\nroblox = false\n",
        )?;

        let mut child = Command::new(EXECUTABLE)
            .args([
                "--debug", "--color", "never", "check", "--watch", "--mode", "strict",
            ])
            .arg("--root")
            .arg(root)
            .arg("--configuration")
            .arg(root.join("custom.toml"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        let stream = child.stderr.take().expect("watcher stderr");
        let (sender, output) = mpsc::channel();

        let reader = thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        let session = Self {
            child,
            output,
            reader: Some(reader),
        };

        let output = session.batch();
        assert!(output.contains("checks=Some(2)"), "{output}");
        assert!(!output.contains("error:"), "{output}");

        Ok(session)
    }

    fn batch(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut output = String::new();

        loop {
            let line = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("watcher did not complete: {error}\n{output}"))
                .expect("read watcher output");

            output.push_str(&line);
            output.push('\n');

            if line == "Watching for changes..." {
                return output;
            }
        }
    }

    fn quiet(&self) {
        let deadline = Instant::now() + Duration::from_millis(800);
        let mut output = String::new();

        loop {
            match self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(line) => {
                    output.push_str(&line.expect("read watcher output"));
                    output.push('\n');
                    assert!(!output.contains("Processing batch"), "{output}");
                    assert!(!output.contains("Creating native frontend"), "{output}");
                }

                Err(RecvTimeoutError::Timeout) => return,
                Err(error) => panic!("watcher stopped: {error}\n{output}"),
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.child.try_wait().expect("watcher status").is_none() {
            self.child.kill().expect("stop watcher");
        }

        self.child.wait().expect("wait for watcher");

        self.reader
            .take()
            .expect("watcher reader")
            .join()
            .expect("join watcher reader");
    }
}

fn checks(output: &str, expected: usize) {
    assert!(!output.contains("Creating native frontend"), "{output}");

    assert!(
        output.contains(&format!("checks=Some({expected})")),
        "{output}"
    );

    let noun = if expected == 1 { "file" } else { "files" };

    assert!(
        output.contains(&format!("Checked {expected} {noun} in ")),
        "{output}"
    );

    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("Checking  "))
            .count(),
        expected,
        "{output}"
    );
}

#[test]
fn irrelevant() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join(".git"))?;
    fs::create_dir(root.join("ignored"))?;
    fs::write(root.join("source.luau"), "return 1\n")?;
    fs::write(root.join("independent.luau"), "return 2\n")?;
    let session = Session::start(root)?;

    session.quiet();
    fs::write(root.join(".git/index.lock"), "unrelated")?;
    session.quiet();
    fs::remove_file(root.join(".git/index.lock"))?;
    session.quiet();
    fs::write(root.join("ignored/source.luau"), "return 3\n")?;
    session.quiet();
    fs::write(root.join("source.luau"), "return 1\n")?;
    session.quiet();

    Ok(())
}

#[test]
fn membership() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("source.luau"), "return 1\n")?;
    fs::write(root.join("independent.luau"), "return 2\n")?;
    let session = Session::start(root)?;

    fs::write(
        root.join("added.luau"),
        "local value: number = \"wrong\"\nreturn value\n",
    )?;

    let output = session.batch();
    checks(&output, 1);
    assert!(output.lines().any(|line| line == "1000"), "{output}");
    assert!(output.contains("error:"), "{output}");

    fs::remove_file(root.join("added.luau"))?;
    let output = session.batch();
    checks(&output, 0);
    assert!(!output.contains("error:"), "{output}");

    fs::write(
        root.join("source.luau"),
        "local value: number = require(\"./dependency\")\nreturn value\n",
    )?;

    let output = session.batch();
    checks(&output, 1);
    assert!(output.lines().any(|line| line == "1012"), "{output}");
    assert!(output.contains("error:"), "{output}");

    fs::write(root.join("dependency.luau"), "return 1\n")?;
    let output = session.batch();
    checks(&output, 2);
    assert!(!output.contains("error:"), "{output}");

    fs::write(root.join("dependency.luau"), "return \"wrong\"\n")?;
    let output = session.batch();
    checks(&output, 2);
    assert!(output.lines().any(|line| line == "1000"), "{output}");
    assert!(output.contains("error:"), "{output}");

    fs::rename(root.join("dependency.luau"), root.join("renamed.luau"))?;
    let output = session.batch();
    checks(&output, 2);
    assert!(output.lines().any(|line| line == "1012"), "{output}");
    assert!(output.contains("error:"), "{output}");

    Ok(())
}

#[test]
fn navigation() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("ignored"))?;
    fs::write(root.join("source.luau"), "return 1\n")?;
    fs::write(root.join("independent.luau"), "return 2\n")?;
    let session = Session::start(root)?;

    let requested = if cfg!(windows) { "UPPER" } else { "upper" };

    fs::write(
        root.join("source.luau"),
        format!("local value: number = require(\"./ignored/{requested}\")\nreturn value\n"),
    )?;

    let output = session.batch();
    checks(&output, 1);
    assert!(output.lines().any(|line| line == "1012"), "{output}");
    assert!(output.contains("error:"), "{output}");

    fs::write(root.join("ignored/upper.luau"), "return 1\n")?;
    let output = session.batch();
    checks(&output, 2);
    assert!(!output.contains("error:"), "{output}");

    Ok(())
}
