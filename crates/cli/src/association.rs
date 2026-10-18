use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Component, MAIN_SEPARATOR, Path, PathBuf},
};

use eyre::{Result, WrapErr as _, bail, eyre};
use instar_core::workspace;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::{interrupt, report};

const PATTERN: &str = "**/instar.toml";

#[derive(Copy, Clone, clap::ValueEnum)]
pub(super) enum Association {
    Tombi,
    Taplo,
}

pub(super) struct Configuration {
    pub(super) path: PathBuf,
    original: Option<String>,
    proposed: String,
}

impl Configuration {
    pub(super) fn apply(&self) -> Result<()> {
        interrupt::check()?;

        if self.original.as_deref() == Some(self.proposed.as_str()) {
            let current = fs::read_to_string(&self.path)
                .wrap_err_with(|| format!("cannot read {}", self.path.display()))?;

            if current != self.proposed {
                bail!(
                    "TOML tool configuration `{}` changed during generation",
                    self.path.display()
                );
            }

            return Ok(());
        }

        if let Some(original) = &self.original {
            workspace::replace(&self.path, original, &self.proposed)
                .wrap_err_with(|| format!("cannot update {}", self.path.display()))?;
        } else {
            let directory = self
                .path
                .parent()
                .ok_or_else(|| eyre!("TOML tool configuration has no parent directory"))?;

            let mut staged = tempfile::NamedTempFile::new_in(directory)
                .wrap_err("cannot stage TOML tool configuration")?;

            staged
                .write_all(self.proposed.as_bytes())
                .wrap_err("cannot write TOML tool configuration")?;

            staged
                .as_file()
                .sync_all()
                .wrap_err("cannot flush TOML tool configuration")?;

            interrupt::check()?;

            staged
                .persist_noclobber(&self.path)
                .wrap_err_with(|| format!("cannot create {}", self.path.display()))?;
        }

        Ok(())
    }
}

pub(super) fn prepare(
    directory: &Path,
    association: Association,
    schema: &Path,
) -> Result<Configuration> {
    let (names, field): (&[&str], _) = match association {
        Association::Tombi => (
            &[
                ".tombi.toml",
                "tombi.toml",
                ".config/tombi.toml",
                "pyproject.toml",
            ],
            "schemas",
        ),

        Association::Taplo => (&[".taplo.toml", "taplo.toml"], "rule"),
    };

    let mut path = directory.join(names[0]);
    let mut original = None;
    let mut document = DocumentMut::new();

    for name in names {
        let candidate = directory.join(name);

        let metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,

            Err(error) => {
                return Err(error).wrap_err_with(|| format!("cannot read {}", candidate.display()));
            }
        };

        if !metadata.is_file() {
            bail!(
                "TOML tool configuration `{}` must be a regular file",
                candidate.display()
            );
        }

        let contents = fs::read_to_string(&candidate)
            .wrap_err_with(|| format!("cannot read {}", candidate.display()))?;

        let parsed = contents
            .parse::<DocumentMut>()
            .wrap_err_with(|| format!("cannot parse {}", candidate.display()))?;

        if *name == "pyproject.toml"
            && parsed
                .get("tool")
                .and_then(|tool| tool.get("tombi"))
                .is_none()
        {
            continue;
        }

        if metadata.permissions().readonly() {
            bail!(
                "TOML tool configuration `{}` is read-only",
                candidate.display()
            );
        }

        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&candidate)
            .wrap_err_with(|| format!("cannot update {}", candidate.display()))?
            .try_lock()
            .map_err(io::Error::other)
            .wrap_err_with(|| format!("cannot lock {}", candidate.display()))?;

        path = candidate;
        original = Some(contents);
        document = parsed;
        break;
    }

    let directory = normalize(directory);
    let schema = normalize(schema);
    let address = report::relative(&schema, &directory);

    let address = address
        .to_str()
        .ok_or_else(|| eyre!("generated schema path must be valid UTF-8"))?
        .replace(MAIN_SEPARATOR, "/");

    let configuration: &mut dyn TableLike = if path.ends_with("pyproject.toml") {
        document
            .get_mut("tool")
            .and_then(|tool| tool.get_mut("tombi"))
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| eyre!("cannot associate schema: tool.tombi must be a table"))?
    } else {
        document.as_table_mut()
    };

    let entries = configuration
        .entry(field)
        .or_insert_with(|| toml_edit::value(Array::new()));

    configure(entries, association, &address)
        .wrap_err_with(|| format!("cannot update {field} in {}", path.display()))?;

    let proposed = document.to_string();

    Ok(Configuration {
        path,
        original,
        proposed,
    })
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            _ => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

fn configure(entries: &mut Item, association: Association, address: &str) -> Result<()> {
    match entries {
        Item::ArrayOfTables(entries) => {
            let selected = select(
                entries.iter().map(|entry| entry as &dyn TableLike),
                association,
            );

            let mut entry = selected.map_or_else(Table::new, |index| entries.remove(index));

            configure_entry(&mut entry, association, address)?;

            match association {
                Association::Tombi => {
                    order(&mut entry, 0);
                    entries.insert(0, entry);
                }

                Association::Taplo => {
                    order(&mut entry, isize::MAX);
                    entries.push(entry);
                }
            }
        }

        Item::Value(Value::Array(entries)) => {
            if !entries.iter().all(Value::is_inline_table) {
                bail!("expected an array of tables");
            }

            let selected = select(
                entries
                    .iter()
                    .filter_map(Value::as_inline_table)
                    .map(|entry| entry as &dyn TableLike),
                association,
            );

            let mut entry = selected.map_or_else(
                || Value::InlineTable(InlineTable::new()),
                |index| entries.remove(index),
            );

            configure_entry(
                entry
                    .as_inline_table_mut()
                    .ok_or_else(|| eyre!("expected an inline table"))?,
                association,
                address,
            )?;

            match association {
                Association::Tombi => entries.insert_formatted(0, entry),
                Association::Taplo => entries.push_formatted(entry),
            }
        }

        _ => bail!("expected an array of tables"),
    }

    Ok(())
}

fn select<'entry>(
    entries: impl Iterator<Item = &'entry dyn TableLike>,
    association: Association,
) -> Option<usize> {
    let mut matching = entries.enumerate().filter_map(|(index, entry)| {
        let include = entry.get("include")?.as_array()?;

        (include.len() == 1
            && include.get(0)?.as_str() == Some(PATTERN)
            && ["exclude", "root", "keys"]
                .iter()
                .all(|key| !entry.contains_key(key)))
        .then_some(index)
    });

    match association {
        Association::Tombi => matching.next(),
        Association::Taplo => matching.last(),
    }
}

fn configure_entry(
    entry: &mut dyn TableLike,
    association: Association,
    address: &str,
) -> Result<()> {
    entry
        .entry("include")
        .or_insert_with(|| toml_edit::value(Array::from_iter([PATTERN])));

    let schema = match association {
        Association::Tombi => entry,

        Association::Taplo => entry
            .entry("schema")
            .or_insert_with(|| toml_edit::value(InlineTable::new()))
            .as_table_like_mut()
            .ok_or_else(|| eyre!("schema must be a table"))?,
    };

    let path = schema.entry("path").or_insert(Item::None);

    if path.as_str() != Some(address) {
        let mut value = Value::from(address);

        if let Some(previous) = path.as_value() {
            *value.decor_mut() = previous.decor().clone();
        }

        *path = Item::Value(value);
    }

    Ok(())
}

fn order(table: &mut Table, position: isize) {
    table.set_position(Some(position));

    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(child) => order(child, position),

            Item::ArrayOfTables(children) => {
                for child in children.iter_mut() {
                    order(child, position);
                }
            }

            _ => {}
        }
    }
}
