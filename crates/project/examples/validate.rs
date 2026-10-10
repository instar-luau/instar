//! Runs project analysis and editor operations over representative fixtures.

use std::{
    env, fs,
    future::Future,
    io,
    path::{self, Path},
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
    time::{Duration, Instant},
};

use instar_project::{
    ChangeBatch, CheckRequest, Input, Invocation, Project, ProjectChange, Query, QueryOutput,
    QueryRequest, SourceChange, SourceOrigin, documentation_cache_directory,
    roblox_cache_directory,
};

use instar_shared::{Completion, Options};

fn main() -> io::Result<()> {
    let argument = env::args_os()
        .nth(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected asset directory"))?;

    let destination = path::absolute(Path::new(&argument))?;
    let directory = destination.join("roblox");

    for profile in ["none", "local", "plugin", "roblox"] {
        let project = Project::new(vec![directory.clone()]);
        let mut changes = Vec::new();

        for (cache, bundle, names) in [
            (
                roblox_cache_directory()?,
                directory.clone(),
                vec![
                    "metadata.json",
                    "documentation.json",
                    "enumerations.d.luau",
                    "none.d.luau",
                    "local.d.luau",
                    "plugin.d.luau",
                    "roblox.d.luau",
                ],
            ),
            (
                documentation_cache_directory()?,
                destination.join("luau"),
                vec!["metadata.json", "documentation.json"],
            ),
        ] {
            for name in names {
                let file = cache.join(name);

                changes.push(ProjectChange::Source(SourceChange::Open {
                    identity: project.source_identity(),
                    origin: SourceOrigin::Document {
                        address: file.to_string_lossy().into_owned(),
                        context: Some(file),
                    },
                    text: Arc::from(fs::read_to_string(bundle.join(name))?),
                }));
            }
        }

        let configuration = directory.join("instar.toml");

        changes.push(ProjectChange::Source(SourceChange::Open {
            identity: project.source_identity(),
            origin: SourceOrigin::Document {
                address: configuration.to_string_lossy().into_owned(),
                context: Some(configuration),
            },
            text: Arc::from(format!("[environment]\nroblox=true\nsecurity='{profile}'")),
        }));

        let entry = directory.join("validation.luau");
        let identity = project.source_identity();

        changes.push(ProjectChange::Source(SourceChange::Open {
            identity,
            origin: SourceOrigin::Document {
                address: entry.to_string_lossy().into_owned(), context: Some(entry),
            },
            text: Arc::from("--!strict\nlocal part: Part = Instance.new('Part')\nlocal service: Players = game:GetService('Players')\nlocal material: Enum.Material = Enum.Material.Plastic\nreturn part, service, material"),
        }));

        project.change(ChangeBatch { changes })?;

        let checked = complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Source(identity)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            Options::new(Some(Instant::now() + Duration::from_secs(30))),
        ))
        .map_err(io::Error::other)?;

        if checked.completion != Completion::Complete
            || !checked.findings.is_empty()
            || !checked.environment_diagnostics.is_empty()
        {
            return Err(io::Error::other(format!("{profile}: {checked:?}")));
        }

        let source = checked
            .snapshot
            .source(identity)
            .expect("validation source")
            .version;

        let documentation = complete(project.query(
            QueryRequest {
                snapshot: checked.snapshot,
                operation: Query::Documentation {
                    source,
                    symbol: "@roblox/global/game".to_owned(),
                },
            },
            Options::new(Some(Instant::now() + Duration::from_secs(30))),
        ))
        .map_err(io::Error::other)?;

        if documentation.completion != Completion::Complete
            || !documentation.diagnostics.is_empty()
            || !matches!(documentation.output, QueryOutput::Documentation(Some(_)))
        {
            return Err(io::Error::other(format!("{profile}: {documentation:?}")));
        }

        project.stop();
        complete(project.finish())?;
    }

    Ok(())
}

fn complete<Output>(future: impl Future<Output = Output>) -> Output {
    struct Notification(thread::Thread);

    impl Wake for Notification {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(Notification(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}
