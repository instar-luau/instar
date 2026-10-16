use std::path::Path;

use figment::{Error, Figment, Metadata, value::Value};

pub(super) struct Source<'data> {
    figment: &'data Figment,
    value: Option<&'data Value>,
    metadata: Option<&'data Metadata>,
    path: Vec<String>,
}

impl<'data> Source<'data> {
    pub(super) fn new(figment: &'data Figment, value: &'data Value) -> Self {
        Self {
            figment,
            value: Some(value),
            metadata: figment.get_metadata(value.tag()),
            path: Vec::new(),
        }
    }

    pub(super) fn field(&self, name: &str) -> Self {
        let value = self.value.and_then(|value| match value {
            Value::Dict(_, values) => values.get(name),

            Value::Array(_, values) => name
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index)),

            _ => None,
        });

        let mut path = self.path.clone();
        path.push(name.to_owned());

        Self {
            figment: self.figment,
            value,
            metadata: value
                .and_then(|value| self.figment.get_metadata(value.tag()))
                .or(self.metadata),
            path,
        }
    }

    pub(super) fn base<'base>(&'base self, fallback: &'base Path) -> &'base Path {
        self.metadata
            .and_then(|metadata| metadata.source.as_ref())
            .and_then(|source| source.file_path())
            .and_then(Path::parent)
            .unwrap_or(fallback)
    }

    pub(super) fn error(&self, message: impl Into<String>) -> Box<Error> {
        let message = message.into();
        let mut error = Error::from(message);
        error.path.clone_from(&self.path);
        error.metadata = self.metadata.cloned();

        Box::new(error)
    }
}
