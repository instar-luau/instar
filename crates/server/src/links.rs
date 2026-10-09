use instar_analysis::error::invalid;
use instar_core::links::Target;

use tower_lsp_server::{
    gen_lsp_types::{DocumentLink, DocumentLinkParams, Uri},
    jsonrpc::Result,
};

use crate::{location, position, session::Session};

impl Session {
    pub(crate) async fn links(&self, parameters: DocumentLinkParams) -> Result<Vec<DocumentLink>> {
        let tooltips = self.workspace.lock().expect("workspace lock").link_tooltips;

        self.query(
            &parameters.text_document.uri,
            &self.workers,
            move |path, encoding, project, options, started| {
                let source = project.source(path)?;
                let mut result = Vec::new();

                for link in project.document_links(path, &options.remaining(started))? {
                    options.check(started)?;

                    if link.location.revision != source.revision {
                        return Err(invalid("document link source revision mismatch"));
                    }

                    let target = match link.target {
                        Target::Source(path) => location::uri(&path)?,

                        Target::Documentation(address) => match address.parse::<Uri>() {
                            Ok(target)
                                if matches!(target.scheme(), "http" | "https")
                                    && target.host_str().is_some() =>
                            {
                                target
                            }

                            _ => continue,
                        },
                    };

                    let tooltip = tooltips.then(|| target.as_str().to_owned());

                    result.push(DocumentLink {
                        range: position::range(&source.text, link.location.range, encoding)?,
                        target: Some(target),
                        tooltip,
                        data: None,
                    });
                }

                Ok(result)
            },
        )
        .await
    }
}
