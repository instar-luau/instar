use serde::Deserialize;
use tower_lsp_server::gen_lsp_types::{Position, PositionEncodingKind};

use crate::position;

#[derive(Debug, Deserialize)]
struct PositionCase {
    encoding: PositionEncodingKind,
    source: String,
    position: Position,
    offset: Option<usize>,
    end: Position,
}

#[test]
fn positions_cover_unicode_crlf_and_code_unit_boundaries() {
    let cases: Vec<PositionCase> = serde_json::from_str(include_str!("../fixtures/positions.json"))
        .expect("position fixtures");

    for case in cases {
        assert_eq!(
            position::offset(&case.source, case.position, &case.encoding).ok(),
            case.offset,
            "{case:?}"
        );

        assert_eq!(position::end(&case.source, &case.encoding), case.end);
    }
}
