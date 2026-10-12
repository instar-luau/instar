use std::{io, path::Path, sync::Arc};

use instar_shared::Options;

use crate::{
    capture::Capture, execution::Runtime, report::SettingsReport, request::SettingsRequest,
    selection::SelectionPurpose,
};

impl Runtime {
    pub(crate) fn settings(
        &mut self,
        request: SettingsRequest,
        options: &Options,
    ) -> io::Result<SettingsReport> {
        let mut capture = request.snapshot.map_or_else(
            || Capture::live(Arc::clone(&self.host)),
            |snapshot| Capture::new(Arc::clone(&self.host), snapshot),
        );

        let selected = self.select(
            &request.invocation,
            &request.overrides,
            &mut capture,
            SelectionPurpose::Settings,
            options,
            |_, _, _, _| Ok(()),
        )?;

        let mut settings = selected
            .targets
            .into_iter()
            .map(|target| target.effective)
            .collect::<Vec<_>>();

        settings.sort_by_key(|effective| {
            capture
                .snapshot
                .origin(effective.source)
                .and_then(|origin| origin.path())
                .map(Path::to_path_buf)
        });

        let inputs = capture.guards();
        options.check()?;

        Ok(SettingsReport {
            snapshot: capture.snapshot,
            inputs,
            operation: request.overrides.operation(),
            settings,
            schema_identity: "instar.project.settings.v1".to_owned(),
            diagnostics: Vec::new(),
            completion: selected.completion,
        })
    }
}
