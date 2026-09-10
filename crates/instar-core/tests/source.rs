use std::{error::Error, fs, sync::Arc};

use instar_core::source::{PositionEncoding, SourceError, SourceStore};
use line_index::LineCol;
use text_size::{TextRange, TextSize};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn editor_revisions_override_disk_until_closed() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("main.luau");
    fs::write(&path, "disk")?;
    let mut store = SourceStore::default();
    let disk = store.read(&path)?;
    assert!(Arc::ptr_eq(&disk, &store.read(&path)?));
    let editor = store.open(&path, 1, "editor")?;
    assert!(matches!(store.validate(&disk), Err(SourceError::Stale)));
    fs::write(&path, "changed disk")?;
    assert!(Arc::ptr_eq(&editor, &store.read(&path)?));
    assert!(matches!(
        store.open(&path, 2, "replacement"),
        Err(SourceError::AlreadyOpen)
    ));
    assert!(matches!(
        store.update(&editor, 1, "replacement"),
        Err(SourceError::Version)
    ));
    let changed = store.update(&editor, 2, "changed editor")?;
    assert_eq!(editor.text()?, "editor");
    assert!(matches!(
        store.update(&editor, 3, "stale"),
        Err(SourceError::Stale)
    ));
    assert!(matches!(store.close(&editor), Err(SourceError::Stale)));
    let restored = store.update(&changed, 3, "editor")?;
    assert_ne!(editor.revision(), restored.revision());
    let mut other_store = SourceStore::default();
    let other = other_store.open(&path, 3, "editor")?;
    assert!(matches!(store.validate(&other), Err(SourceError::Stale)));
    store.close(&restored)?;
    assert!(store.validate(&restored).is_err());
    let disk = store.read(&path)?;
    assert_eq!(disk.text()?, "changed disk");
    assert!(matches!(
        store.update(&disk, 4, "text"),
        Err(SourceError::NotOpen)
    ));
    assert!(matches!(store.close(&disk), Err(SourceError::NotOpen)));
    fs::remove_file(&path)?;
    assert!(store.read(&path).is_err());
    let virtual_source = store.open(&path, 1, "unsaved")?;
    assert!(Arc::ptr_eq(&virtual_source, &store.read(&path)?));
    Ok(())
}

#[test]
fn malformed_bytes_have_stable_position_units() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("positions.luau");
    let bytes = ["é😀".as_bytes(), b"\xff\xe2\x82\r\nx"].concat();
    fs::write(&path, &bytes)?;
    let source = SourceStore::default().read(&path)?;
    for (offset, column) in [(0, 0), (2, 1), (6, 3), (7, 4), (8, 5), (9, 6)] {
        let position = LineCol {
            line: 0,
            col: column,
        };
        assert_eq!(
            source.position(offset.into(), PositionEncoding::Utf16)?,
            position
        );
        assert_eq!(
            source.offset(position, PositionEncoding::Utf16)?,
            offset.into()
        );
    }
    assert!(source.position(1.into(), PositionEncoding::Utf16).is_err());
    assert!(source.position(10.into(), PositionEncoding::Utf16).is_err());
    assert_eq!(
        source.position(11.into(), PositionEncoding::Utf16)?,
        LineCol { line: 1, col: 0 }
    );
    assert_eq!(source.bytes(), bytes);
    Ok(())
}

#[test]
fn disk_bytes_and_old_snapshots_remain_exact() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("bytes.luau");
    fs::write(&path, b"return '\xff'")?;
    let mut store = SourceStore::default();
    assert!(matches!(
        store.read(root.path()),
        Err(SourceError::NotFile(_))
    ));
    let source = store.read(&path)?;
    assert_eq!(source.bytes(), b"return '\xff'");
    let tree = source.parse();
    assert_eq!(tree.source, source.bytes());
    assert!(std::ptr::eq(tree.source.as_ptr(), source.bytes().as_ptr()));
    assert!(matches!(source.text(), Err(SourceError::Encoding(_))));
    assert_eq!(
        source.position(TextSize::from(9), PositionEncoding::Utf16)?,
        LineCol { line: 0, col: 9 }
    );
    assert_eq!(
        source.offset(LineCol { line: 0, col: 9 }, PositionEncoding::Utf16)?,
        TextSize::from(9)
    );
    assert_eq!(source.slice(TextRange::new(8.into(), 9.into()))?, &[255]);
    assert!(source.slice(TextRange::new(0.into(), 100.into())).is_err());
    fs::write(&path, "new")?;
    let new = store.read(&path)?;
    assert_ne!(new.revision(), source.revision());
    assert!(store.validate(&source).is_err());
    assert_eq!(source.bytes(), b"return '\xff'");
    assert_eq!(tree.source, source.bytes());
    assert_eq!(new.parse().source, new.bytes());
    Ok(())
}

#[test]
fn positions_validate_unicode_crlf_and_bounds() -> TestResult {
    let mut store = SourceStore::default();
    let root = tempfile::tempdir()?;
    let source = store.open(&root.path().join("text.luau"), 1, "a😀\r\nβ\n")?;
    assert_eq!(
        source.position(5.into(), PositionEncoding::Utf16)?,
        LineCol { line: 0, col: 3 }
    );
    assert_eq!(
        source.offset(LineCol { line: 1, col: 1 }, PositionEncoding::Utf16)?,
        TextSize::from(9)
    );
    for encoding in [
        PositionEncoding::Utf8,
        PositionEncoding::Utf16,
        PositionEncoding::Utf32,
    ] {
        for offset in 0..=10 {
            let offset = TextSize::from(offset);
            if let Ok(position) = source.position(offset, encoding) {
                assert_eq!(source.offset(position, encoding)?, offset);
            }
        }
        assert!(source.position(6.into(), encoding).is_err());
        assert!(source.position(2.into(), encoding).is_err());
        assert!(source.position(11.into(), encoding).is_err());
        assert!(
            source
                .offset(
                    LineCol {
                        line: 0,
                        col: u32::MAX
                    },
                    encoding
                )
                .is_err()
        );
        assert!(
            source
                .offset(
                    LineCol {
                        line: u32::MAX,
                        col: 0
                    },
                    encoding
                )
                .is_err()
        );
    }
    assert!(
        source
            .offset(LineCol { line: 0, col: 2 }, PositionEncoding::Utf16)
            .is_err()
    );
    assert!(
        source
            .offset(LineCol { line: 0, col: 7 }, PositionEncoding::Utf8)
            .is_err()
    );
    assert!(
        source
            .offset(LineCol { line: 2, col: 1 }, PositionEncoding::Utf8)
            .is_err()
    );
    Ok(())
}
