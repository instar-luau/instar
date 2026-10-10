use instar_project::ProjectSnapshot;
use instar_shared::Location;

use tower_lsp_server::{
    gen_lsp_types::{Location as ProtocolLocation, Range},
    jsonrpc::{Error, Result},
};

use crate::{
    Server, position,
    session::{State, failure},
};

impl Server {
    pub(crate) fn location(
        &self,
        snapshot: &ProjectSnapshot,
        location: Location,
    ) -> Result<ProtocolLocation> {
        self.state
            .lock()
            .expect("session state")
            .location(snapshot, location)
    }

    pub(crate) fn range(&self, snapshot: &ProjectSnapshot, location: Location) -> Result<Range> {
        self.location(snapshot, location)
            .map(|location| location.range)
    }
}

impl State {
    pub(crate) fn location(
        &self,
        snapshot: &ProjectSnapshot,
        location: Location,
    ) -> Result<ProtocolLocation> {
        let source = snapshot
            .source(location.source.identity)
            .ok_or_else(Error::content_modified)?;

        location.validate(source).map_err(|error| failure(&error))?;

        Ok(ProtocolLocation {
            uri: self.uri(snapshot, location.source.identity)?,
            range: position::range(
                &source.text,
                [location.span.start, location.span.end],
                &self.encoding,
            )
            .map_err(|error| failure(&error))?,
        })
    }
}
