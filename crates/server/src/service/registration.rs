use std::collections::BTreeSet;

use serde_json::json;

use tower_lsp_server::{
    Client,
    gen_lsp_types::{Registration, Unregistration},
    jsonrpc::Result,
};

use super::protocol::Service;
use crate::project::Project;

#[derive(Default)]
pub(super) struct Registrations {
    pub(super) installed: Vec<Registration>,
    pub(super) is_stopping: bool,
}

impl Registrations {
    pub(super) async fn unregister(&mut self, client: &Client) -> Result<()> {
        if !self.installed.is_empty() {
            client
                .unregister_capability(
                    self.installed
                        .iter()
                        .map(|registration| Unregistration {
                            id: registration.id.clone(),
                            method: registration.method.clone(),
                        })
                        .collect(),
                )
                .await?;

            self.installed.clear();
        }

        Ok(())
    }
}

impl Service {
    pub(super) async fn register(&self) -> Result<()> {
        let mut acknowledged = self.registrations.lock().await;

        if acknowledged.is_stopping {
            return Ok(());
        }

        let mut registrations = Vec::new();

        if self.has_capability("/workspace/didChangeWatchedFiles/dynamicRegistration") {
            let dependencies = self
                .connection
                .control(|state| {
                    Ok(state
                        .projects
                        .values()
                        .flat_map(Project::dependencies)
                        .collect::<BTreeSet<_>>())
                })
                .await?;

            let mut watchers = vec![json!({"globPattern":"**/*.{lua,luau,toml,json}"})];

            watchers.extend(dependencies.into_iter().filter_map(|path| {
                path.to_str()
                    .map(|path| json!({"globPattern":globset::escape(&path.replace('\\', "/"))}))
            }));

            registrations.push(Registration {
                id: "instar.files".to_owned(),
                method: "workspace/didChangeWatchedFiles".to_owned(),
                register_options: Some(json!({"watchers":watchers})),
            });
        }

        if self.has_capability("/workspace/didChangeConfiguration/dynamicRegistration") {
            registrations.push(Registration {
                id: "instar.configuration".to_owned(),
                method: "workspace/didChangeConfiguration".to_owned(),
                register_options: None,
            });
        }

        if registrations == acknowledged.installed {
            return Ok(());
        }

        acknowledged.unregister(&self.client).await?;

        if !registrations.is_empty() {
            self.client
                .register_capability(registrations.clone())
                .await?;

            acknowledged.installed = registrations;
        }

        Ok(())
    }
}
