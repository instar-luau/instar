use std::{
    env, fs,
    io::{self, Write as _},
};

use eyre::{Result, WrapErr as _, bail, eyre};
use instar_core::{settings, workspace};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, Value};

const ADDRESS: &str = "https://instar-luau.github.io/instar/schema.json";
const PATTERN: &str = "**/instar.toml";

#[derive(Copy, Clone, clap::ValueEnum)]
pub(super) enum Association {
    Tombi,
    Taplo,
}

pub(super) fn run(association: Option<Association>) -> Result<u8> {
    if let Some(association) = association {
        associate(association)?;

        return Ok(0);
    }

    let mut output = io::stdout().lock();

    serde_json::to_writer_pretty(&mut output, &settings::schema())
        .wrap_err("cannot write configuration schema")?;

    writeln!(output).wrap_err("cannot finish configuration schema")?;

    Ok(0)
}

fn associate(association: Association) -> Result<()> {
    let directory = env::current_dir().wrap_err("cannot find the current directory")?;

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

        let contents = match fs::read_to_string(&candidate) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,

            Err(error) => {
                return Err(error).wrap_err_with(|| format!("cannot read {}", candidate.display()));
            }
        };

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

        path = candidate;
        original = Some(contents);
        document = parsed;
        break;
    }

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

    configure(entries, association)
        .wrap_err_with(|| format!("cannot update {field} in {}", path.display()))?;

    let proposed = document.to_string();

    if original.as_deref() == Some(proposed.as_str()) {
        return Ok(());
    }

    if let Some(original) = original {
        workspace::replace(&path, &original, &proposed)
            .wrap_err_with(|| format!("cannot update {}", path.display()))?;
    } else {
        let mut staged = tempfile::NamedTempFile::new_in(&directory)
            .wrap_err("cannot stage TOML tool configuration")?;

        staged
            .write_all(proposed.as_bytes())
            .wrap_err("cannot write TOML tool configuration")?;

        staged
            .as_file()
            .sync_all()
            .wrap_err("cannot flush TOML tool configuration")?;

        staged
            .persist_noclobber(&path)
            .wrap_err_with(|| format!("cannot create {}", path.display()))?;
    }

    Ok(())
}

fn configure(entries: &mut Item, association: Association) -> Result<()> {
    match entries {
        Item::ArrayOfTables(entries) => {
            let selected = select(
                entries.iter().map(|entry| entry as &dyn TableLike),
                association,
            );

            let mut entry = selected.map_or_else(Table::new, |index| entries.remove(index));

            configure_entry(&mut entry, association)?;

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

fn configure_entry(entry: &mut dyn TableLike, association: Association) -> Result<()> {
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

    if path.as_str() != Some(ADDRESS) {
        let mut value = Value::from(ADDRESS);

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
