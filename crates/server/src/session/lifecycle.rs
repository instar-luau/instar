use std::collections::BTreeSet;

use instar_project::{ChangeBatch, ProjectChange};
use serde_json::json;

use tower_lsp_server::{
    gen_lsp_types::{
        ConfigurationItem, InitializeParams, InitializeResult, InitializedParams, Registration,
        ServerInfo, Uri, WorkspaceFolders,
    },
    jsonrpc::{Error, Result},
};

use super::{Server, capabilities::Capabilities, failure, path, state::Lifecycle};

impl Server {
    pub(crate) fn initialize(&self, parameters: InitializeParams) -> Result<InitializeResult> {
        let roots = match parameters
            .workspace_folders_initialize_params
            .workspace_folders
        {
            Some(WorkspaceFolders::WorkspaceFolderList(folders)) => folders
                .into_iter()
                .map(|folder| path(&folder.uri))
                .collect::<Result<BTreeSet<_>>>()?,

            _ => self.project.snapshot().roots().cloned().collect(),
        };

        let capabilities = Capabilities::new(parameters.capabilities)?;
        let advertised = capabilities.advertised()?;
        let mut state = self.state.lock().expect("session state");

        if state.lifecycle != Lifecycle::Uninitialized {
            return Err(Error::invalid_request());
        }

        let snapshot = self.project.snapshot();
        let existing = snapshot.roots().cloned().collect::<BTreeSet<_>>();

        let changes = ProjectChange::WorkspaceRoots {
            added: roots.difference(&existing).cloned().collect(),
            removed: existing.difference(&roots).cloned().collect(),
        };

        self.project
            .change(ChangeBatch {
                changes: vec![changes],
            })
            .map_err(|error| failure(&error))?;

        state.workspace_folders = roots;
        state.encoding = capabilities.encoding();
        state.capabilities = capabilities;
        state.lifecycle = Lifecycle::Initialized;

        Ok(InitializeResult {
            capabilities: advertised,
            server_info: Some(ServerInfo {
                name: "Instar".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
        })
    }

    pub(crate) async fn initialized(&self, _: InitializedParams) {
        let (watching, configuration, folders) = {
            let state = self.state.lock().expect("session state");

            (
                state
                    .capabilities
                    .supports("/workspace/didChangeWatchedFiles/dynamicRegistration"),
                state.capabilities.supports("/workspace/configuration"),
                state.capabilities.supports("/workspace/workspaceFolders"),
            )
        };

        if watching {
            self.report(
                self.register(Registration {
                    id: "instar-files".to_owned(),
                    method: "workspace/didChangeWatchedFiles".to_owned(),
                    register_options: Some(json!({"watchers":[{"globPattern":"**/*"}]})),
                })
                .await,
            )
            .await;
        }

        if folders {
            let captured = self.project.snapshot().identity;

            match self.client_request(self.client.workspace_folders()).await {
                Ok(Some(folders)) if self.project.snapshot().identity == captured => {
                    let result = folders
                        .into_iter()
                        .map(|folder| path(&folder.uri))
                        .collect::<Result<BTreeSet<_>>>();

                    let result = result.and_then(|roots| {
                        let mut state = self.state.lock().expect("session state");

                        self.project
                            .change(ChangeBatch {
                                changes: vec![ProjectChange::WorkspaceRoots {
                                    added: roots
                                        .difference(&state.workspace_folders)
                                        .cloned()
                                        .collect(),
                                    removed: state
                                        .workspace_folders
                                        .difference(&roots)
                                        .cloned()
                                        .collect(),
                                }],
                            })
                            .map_err(|error| failure(&error))?;

                        state.workspace_folders = roots;

                        Ok(())
                    });

                    self.report(result).await;
                }

                Ok(_) => {}
                Err(error) => self.report(Err(error)).await,
            }
        }

        if configuration {
            let captured = self.project.snapshot();
            let roots = captured.roots().cloned().collect::<Vec<_>>();

            let items = if roots.is_empty() {
                vec![ConfigurationItem {
                    scope_uri: None,
                    section: Some("instar".to_owned()),
                }]
            } else {
                roots
                    .iter()
                    .map(|root| ConfigurationItem {
                        scope_uri: Uri::from_file_path(root).ok(),
                        section: Some("instar".to_owned()),
                    })
                    .collect()
            };

            match self.client_request(self.client.configuration(items)).await {
                Ok(values) if self.project.snapshot().identity == captured.identity => {
                    let result = crate::handlers::configuration_overlays(self, &roots, values);
                    self.report(result).await;
                }

                Ok(_) => {}
                Err(error) => self.report(Err(error)).await,
            }
        }

        self.schedule_diagnostics();
    }

    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.state.lock().expect("session state").shutdown_requested = true;
        self.stop(Lifecycle::Shutdown);

        self.project
            .finish()
            .await
            .map_err(|error| failure(&error))?;

        self.unregister().await
    }
}
