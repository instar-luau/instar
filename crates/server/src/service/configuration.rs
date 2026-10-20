use std::sync::atomic::Ordering;

use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::ConfigurationItem,
    jsonrpc::{Error, Result},
};

use super::protocol::Service;
use crate::{configuration::Configuration, document, error::failure};

impl Service {
    pub(super) async fn configuration(&self, settings: Option<Value>) -> Result<bool> {
        let settings = settings
            .map(|settings| settings.get("instar").cloned().unwrap_or(settings))
            .filter(|settings| !settings.is_null())
            .map(Configuration::load)
            .transpose()?;

        let sequence = self
            .configuration_sequence
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |sequence| {
                sequence.checked_add(1)
            })
            .map_err(|_sequence| Error::internal_error())?;

        let roots = self.connection.control(|state| state.roots()).await?;

        let configurations = if self.has_capability("/workspace/configuration") {
            let items = if roots.is_empty() {
                vec![ConfigurationItem {
                    scope_uri: None,
                    section: Some("instar".to_owned()),
                }]
            } else {
                roots
                    .iter()
                    .map(|uri| ConfigurationItem {
                        scope_uri: Some(uri.clone()),
                        section: Some("instar".to_owned()),
                    })
                    .collect()
            };

            let expected = items.len();
            let values = self.client.configuration(items).await?;

            if values.len() != expected {
                return Err(Error::invalid_params(
                    "Client returned an incorrect number of configurations.",
                ));
            }

            values
                .into_iter()
                .map(|value| {
                    if value.is_null() {
                        Ok(None)
                    } else {
                        Configuration::load(value).map(Some)
                    }
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![None; roots.len().max(1)]
        };

        if settings.is_none() && configurations.iter().all(Option::is_none) {
            return Ok(false);
        }

        self.connection
            .control(move |state| {
                if sequence < state.configuration_revision {
                    return Ok(false);
                }

                let paths = roots
                    .iter()
                    .map(document::path)
                    .collect::<Result<Vec<_>>>()?;

                if paths.iter().any(|path| !state.projects.contains_key(path)) {
                    return Err(Error::content_modified());
                }

                let mut has_changed = false;

                let fallback = if roots.is_empty() {
                    configurations
                        .first()
                        .and_then(Option::as_ref)
                        .or(settings.as_ref())
                } else {
                    settings.as_ref()
                };

                if let Some(configuration) = fallback
                    && serde_json::to_value(&state.configuration).map_err(failure)?
                        != serde_json::to_value(configuration).map_err(failure)?
                {
                    state.configuration = configuration.clone();
                    has_changed = true;
                }

                for (path, configuration) in paths.into_iter().zip(configurations) {
                    let Some(configuration) = configuration.or_else(|| settings.clone()) else {
                        continue;
                    };

                    let project = state
                        .projects
                        .get_mut(&path)
                        .ok_or_else(Error::content_modified)?;

                    if serde_json::to_value(&project.configuration).map_err(failure)?
                        != serde_json::to_value(&configuration).map_err(failure)?
                    {
                        project.changed()?;
                        project.configuration = configuration;
                        has_changed = true;
                    }
                }

                state.configuration_revision = sequence;

                Ok(has_changed)
            })
            .await
    }
}
