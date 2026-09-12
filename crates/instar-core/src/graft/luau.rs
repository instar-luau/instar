#![allow(unsafe_code)]

use super::{RESPONSE_LIMIT, Request};
use serde_json::Value;
use std::{
    ffi::c_void,
    fmt::Write,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    slice,
};

#[repr(C)]
struct Bytes {
    data: *const u8,
    size: usize,
}

impl From<&[u8]> for Bytes {
    fn from(value: &[u8]) -> Self {
        Self {
            data: value.as_ptr(),
            size: value.len(),
        }
    }
}

type Reply = extern "C" fn(*mut c_void, Bytes, bool);

unsafe extern "C" {
    fn instar_graft(
        source: Bytes,
        request: Bytes,
        format: bool,
        lint: bool,
        compile: bool,
        limit: usize,
        context: *mut c_void,
        reply: Reply,
    );
}

extern "C" fn reply(context: *mut c_void, bytes: Bytes, success: bool) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let bytes = if bytes.size == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(bytes.data, bytes.size) }
        };

        if success {
            Ok(bytes.to_vec())
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(bytes).into_owned(),
            ))
        }
    }))
    .unwrap_or_else(|_| Err(io::Error::other("Luau graft callback panicked")));

    unsafe {
        *context.cast::<io::Result<Vec<u8>>>() = result;
    }
}

fn execute(
    source: &[u8],
    request: &[u8],
    format: bool,
    lint: bool,
    compile: bool,
) -> io::Result<Vec<u8>> {
    let mut result: io::Result<Vec<u8>> = Err(io::Error::other("Luau graft returned no response"));

    unsafe {
        instar_graft(
            source.into(),
            request.into(),
            format,
            lint,
            compile,
            RESPONSE_LIMIT,
            (&raw mut result).cast(),
            reply,
        );
    }

    result
}

fn string(value: &str, output: &mut String) {
    output.push('"');

    for byte in value.bytes() {
        write!(output, "\\{byte:03}").expect("writing to a string");
    }

    output.push('"');
}

fn literal(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("nil"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => write!(output, "{value}").expect("writing to a string"),
        Value::String(value) => string(value, output),

        Value::Array(values) => {
            output.push('{');

            for value in values {
                literal(value, output);
                output.push(',');
            }

            output.push('}');
        }

        Value::Object(values) => {
            output.push('{');

            for (key, value) in values {
                output.push('[');
                string(key, output);
                output.push_str("]=");
                literal(value, output);
                output.push(',');
            }

            output.push('}');
        }
    }
}

pub(super) fn validate(source: &[u8], format: bool, lint: bool, compile: bool) -> io::Result<()> {
    execute(source, &[], format, lint, compile).map(|_| ())
}

pub(super) fn invoke(
    source: &[u8],
    request: &Request<'_>,
    format: bool,
    lint: bool,
    compile: bool,
) -> io::Result<Vec<u8>> {
    let request = serde_json::to_value(request).map_err(io::Error::other)?;
    let mut chunk = String::from("return ");
    literal(&request, &mut chunk);

    execute(source, chunk.as_bytes(), format, lint, compile)
}
