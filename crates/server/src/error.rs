use std::fmt::Display;

use tower_lsp_server::jsonrpc::{Error, ErrorCode};

pub(crate) fn failure(error: impl Display) -> Error {
    Error {
        code: ErrorCode::ServerError(-32803),
        message: error.to_string().into(),
        data: None,
    }
}
