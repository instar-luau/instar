use std::{error::Error as Failure, fmt::Display};

use tower_lsp_server::jsonrpc::{Error, ErrorCode};

pub(crate) fn failure(error: impl Failure) -> Error {
    let mut message = error.to_string();
    let mut cause = error.source();

    while let Some(error) = cause {
        message.push('\n');
        message.push_str(&error.to_string());
        cause = error.source();
    }

    reason(message)
}

pub(crate) fn reason(message: impl Display) -> Error {
    Error {
        code: ErrorCode::ServerError(-32803),
        message: message.to_string().into(),
        data: None,
    }
}
