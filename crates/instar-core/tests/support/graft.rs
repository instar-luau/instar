use std::fs;

fn integer(mut value: usize) -> Vec<u8> {
    let mut bytes = Vec::new();

    loop {
        let mut byte = u8::try_from(value & 127).unwrap();
        value >>= 7;

        if value != 0 {
            byte |= 128;
        }

        bytes.push(byte);

        if value == 0 {
            return bytes;
        }
    }
}

fn section(module: &mut Vec<u8>, kind: u8, bytes: &[u8]) {
    module.push(kind);
    module.extend(integer(bytes.len()));
    module.extend(bytes);
}

pub(super) fn module(reply: &str, hook: &str) -> Vec<u8> {
    let mut module = b"\0asm\x01\0\0\0".to_vec();

    section(
        &mut module,
        1,
        &[
            3, 0x60, 1, 0x7f, 1, 0x7f, 0x60, 2, 0x7f, 0x7f, 0, 0x60, 2, 0x7f, 0x7f, 1, 0x7f,
        ],
    );

    section(&mut module, 3, &[3, 0, 1, 2]);
    section(&mut module, 5, &[1, 0, 1]);
    let mut exports = vec![4];

    for (name, kind, index) in [
        ("memory", 2, 0),
        ("instar_allocate", 0, 0),
        ("instar_deallocate", 0, 1),
        (hook, 0, 2),
    ] {
        exports.extend(integer(name.len()));
        exports.extend(name.as_bytes());
        exports.extend([kind, index]);
    }

    section(&mut module, 7, &exports);

    section(
        &mut module,
        10,
        &[
            3, 5, 0, 0x41, 0x80, 0x20, 0x0b, 2, 0, 0x0b, 4, 0, 0x41, 0, 0x0b,
        ],
    );

    let mut payload = Vec::new();
    payload.extend(12u32.to_le_bytes());
    payload.extend(u32::try_from(reply.len()).unwrap().to_le_bytes());
    payload.extend(1u32.to_le_bytes());
    payload.extend(reply.as_bytes());
    let mut data = vec![1, 0, 0x41, 0, 0x0b];
    data.extend(integer(payload.len()));
    data.extend(payload);
    section(&mut module, 11, &data);

    module
}

pub(super) fn luau(source: &str, hook: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("module.luau"), source).unwrap();

    fs::write(
        directory.path().join("graft.toml"),
        format!("name='example'\nversion=1\nruntime='luau'\nentry='module.luau'\n{hook}=true\n"),
    )
    .unwrap();

    directory
}

pub(super) fn fixture(reply: &str, hook: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("module.wasm"), module(reply, hook)).unwrap();

    let capability = if hook == "instar_format" {
        "format"
    } else {
        "lint"
    };

    fs::write(
        directory.path().join("graft.toml"),
        format!("name = 'example'\nversion = 1\nruntime = 'wasm'\nentry = 'module.wasm'\n{capability} = true\n"),
    )
    .unwrap();

    directory
}
