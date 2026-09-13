use super::protocol;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(super) struct Cache {
    revision: u64,
    reports: BTreeMap<PathBuf, (String, Vec<protocol::Diagnostic>)>,
}

impl Cache {
    pub(super) fn report(
        &mut self,
        path: &Path,
        items: Vec<protocol::Diagnostic>,
        previous: Option<&str>,
    ) -> protocol::DocumentDiagnosticReport {
        if self
            .reports
            .get(path)
            .is_none_or(|(_, cached)| *cached != items)
        {
            self.revision = self.revision.wrapping_add(1);

            self.reports
                .insert(path.to_owned(), (self.revision.to_string(), items));
        }

        let (identifier, items) = &self.reports[path];

        if previous == Some(identifier.as_str()) {
            protocol::DocumentDiagnosticReport::Unchanged(
                protocol::RelatedUnchangedDocumentDiagnosticReport {
                    related_documents: None,
                    unchanged_document_diagnostic_report:
                        protocol::UnchangedDocumentDiagnosticReport {
                            result_id: identifier.clone(),
                        },
                },
            )
        } else {
            protocol::DocumentDiagnosticReport::Full(
                protocol::RelatedFullDocumentDiagnosticReport {
                    related_documents: None,
                    full_document_diagnostic_report: protocol::FullDocumentDiagnosticReport {
                        result_id: Some(identifier.clone()),
                        items: items.clone(),
                    },
                },
            )
        }
    }
}
