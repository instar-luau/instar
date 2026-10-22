use std::path::Path;

use notify::{
    Config, Event, EventHandler, EventKind, RecommendedWatcher, RecursiveMode, Result, Watcher,
    WatcherKind, event::Flag,
};

pub(super) struct Events(RecommendedWatcher);

impl Watcher for Events {
    fn new<Handler: EventHandler>(mut handler: Handler, configuration: Config) -> Result<Self> {
        RecommendedWatcher::new(
            move |result: Result<Event>| {
                handler.handle_event(result.map(|event| {
                    if event.paths.is_empty() || matches!(event.kind, EventKind::Other) {
                        event.set_flag(Flag::Rescan)
                    } else {
                        event
                    }
                }));
            },
            configuration,
        )
        .map(Self)
    }

    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> Result<()> {
        self.0.watch(path, mode)
    }

    fn unwatch(&mut self, path: &Path) -> Result<()> {
        self.0.unwatch(path)
    }

    fn configure(&mut self, configuration: Config) -> Result<bool> {
        self.0.configure(configuration)
    }

    fn kind() -> WatcherKind {
        RecommendedWatcher::kind()
    }
}
