use std::{io, path::Path};

use instar_analysis::Location;
use instar_core::{analysis::Origin, project::Project};

use tower_lsp_server::gen_lsp_types::{
    Location as ProtocolLocation, PositionEncodingKind, Range, Uri,
};

use crate::position;

pub(crate) fn convert(
    location: &Location<Origin>,
    project: &mut Project,
    encoding: &PositionEncodingKind,
) -> io::Result<ProtocolLocation> {
    let uri = uri(location.module.source())?;

    if location.revision == 0 && location.range == [0, 0] {
        return Ok(ProtocolLocation {
            uri,
            range: Range::default(),
        });
    }

    let document = project.source(location.module.source())?;

    if document.revision != location.revision {
        return Err(io::Error::other("source revision mismatch"));
    }

    Ok(ProtocolLocation {
        uri,
        range: position::range(&document.text, location.range, encoding)?,
    })
}

pub(crate) fn uri(path: &Path) -> io::Result<Uri> {
    Uri::from_file_path(path).map_err(|()| io::Error::other("invalid source URI"))
}
