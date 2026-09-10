use std::{fs, process::Command, sync::OnceLock};

pub(super) fn fixture(response: &str) -> tempfile::TempDir {
    static COMPILED: OnceLock<tempfile::TempDir> = OnceLock::new();

    let compiled = COMPILED.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();

        fs::write(
            directory.path().join("graft.rs"),
            r#"
use std::{fs, io::{self, Read, Write}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let directory = executable.parent().ok_or("executable has no parent")?;
    let mode = fs::read_to_string(directory.join("mode")).unwrap_or_default();
    if mode == "failure" {
        return Err("native fixture failure".into());
    }
    if mode == "oversize" {
        io::copy(&mut io::repeat(b'x').take(64 * 1024 * 1024 + 1), &mut io::stdout())?;
        return Ok(());
    }
    let response = fs::read(directory.join("response.json"))?;
    if mode == "early" {
        io::stdout().write_all(&response)?;
        io::stdout().flush()?;
    }
    let request = io::read_to_string(io::stdin())?;
    if let Ok(expected) = fs::read_to_string(directory.join("request.json")) {
        assert_eq!(request, expected);
    }
    if mode != "early" {
        io::stdout().write_all(&response)?;
    }
    Ok(())
}
"#,
        )
        .unwrap();

        let output = Command::new("rustc")
            .arg(directory.path().join("graft.rs"))
            .arg("-o")
            .arg(
                directory
                    .path()
                    .join(format!("graft{}", std::env::consts::EXE_SUFFIX)),
            )
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        directory
    });

    let directory = tempfile::tempdir().unwrap();
    let entry = format!("graft{}", std::env::consts::EXE_SUFFIX);
    fs::copy(compiled.path().join(&entry), directory.path().join(&entry)).unwrap();
    fs::write(directory.path().join("response.json"), response).unwrap();

    fs::write(
        directory.path().join("graft.toml"),
        format!(
            "name='example'\nversion=1\nruntime='native'\nentry='{entry}'\nformat=true\nlint=true\n"
        ),
    )
    .unwrap();

    directory
}
