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
    pub(super) async fn unregister(
        &mut self,
        client: &Client,
        should_remove: impl Fn(&Registration) -> bool,
    ) -> Result<()> {
        let removals: Vec<_> = self
            .installed
            .iter()
            .filter(|registration| should_remove(registration))
            .map(|registration| Unregistration {
                id: registration.id.clone(),
                method: registration.method.clone(),
            })
            .collect();

        if !removals.is_empty() {
            client.unregister_capability(removals).await?;

            self.installed
                .retain(|registration| !should_remove(registration));
        }

        Ok(())
    }
}

impl Service {
    pub(super) async fn register(&self, refresh_links: bool) -> Result<()> {
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

            let mut watchers = vec![
                json!({"globPattern":"**/*.{lua,luau,toml,json}"}),
                json!({"globPattern":"**/.luaurc"}),
            ];

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

        if self.has_capability("/textDocument/documentLink/dynamicRegistration") {
            registrations.push(Registration {
                id: "instar.links".to_owned(),
                method: "textDocument/documentLink".to_owned(),
                register_options: Some(json!({
                    "documentSelector": [{"language":"lua"}, {"language":"luau"}]
                })),
            });
        }

        acknowledged
            .unregister(&self.client, |registration| {
                !registrations.contains(registration)
                    || (refresh_links && registration.method == "textDocument/documentLink")
            })
            .await?;

        registrations.retain(|registration| !acknowledged.installed.contains(registration));

        if !registrations.is_empty() {
            self.client
                .register_capability(registrations.clone())
                .await?;

            acknowledged.installed.extend(registrations);
        }

        Ok(())
    }
}
