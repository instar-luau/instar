use std::{io, path::PathBuf};

use instar_lint::RuleProducer;
use instar_shared::{ContextIdentity, Location, SourceIdentity};
use serde::Serialize;

use super::settings::{DiagnosticPolicy, ProjectSettings, ProjectSettingsPatch, canonical_flags};
use crate::inputs::InputVersion;

/// Final field attribution after validated configuration precedence.
#[derive(Clone, Debug, Serialize)]
pub struct SettingOrigin {
    /// Defining input, absent for defaults, client data, and invocation controls.
    pub input: Option<SourceIdentity>,

    /// Canonical settings field path.
    pub field: String,

    /// Exact defining source revision and span when available.
    pub location: Option<Location>,

    /// Directory anchoring relative patterns and paths.
    pub base: PathBuf,

    /// Layer precedence; zero identifies defaults and the maximum identifies invocation controls.
    pub precedence: usize,
}

/// Validated settings for one exact project source and context.
#[derive(Clone, Debug, Serialize)]
pub struct EffectiveSettings {
    /// Selected project context.
    pub context: ContextIdentity,

    /// Selected source identity.
    pub source: SourceIdentity,

    /// Complete effective settings.
    pub settings: ProjectSettings,

    /// Actual request reporting policy, absent for formatting.
    pub diagnostics: Option<DiagnosticPolicy>,

    /// Final field origins, including effective defaults.
    pub origins: Vec<SettingOrigin>,

    /// Every consulted input guarding these settings.
    pub inputs: Vec<InputVersion>,
}

pub(crate) fn record(
    patch: &ProjectSettingsPatch,
    origin: &SettingOrigin,
    origins: &mut Vec<SettingOrigin>,
) -> io::Result<()> {
    let mut patch = patch.clone();
    patch.language.flags = canonical_flags(&patch.language.flags)?;

    patch.resolution.aliases = patch
        .resolution
        .aliases
        .into_iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect();

    let value = serde_json::to_value(patch).map_err(io::Error::other)?;
    collect(&value, "", origin, origins, true);

    Ok(())
}

pub(crate) fn defaults(
    settings: &ProjectSettings,
    origin: &SettingOrigin,
    origins: &mut Vec<SettingOrigin>,
) -> io::Result<()> {
    let mut value = serde_json::to_value(settings).map_err(io::Error::other)?;

    let type_errors = value
        .as_object_mut()
        .and_then(|fields| fields.remove("type_errors"));

    if let Some(mode) = value.get_mut("checking") {
        *mode = serde_json::json!({"mode": mode.clone(), "type_errors": type_errors});
    }

    collect(&value, "", origin, origins, false);

    Ok(())
}

pub(crate) fn resolved(
    patch: &ProjectSettingsPatch,
    settings: &ProjectSettings,
    origin: &SettingOrigin,
    origins: &mut Vec<SettingOrigin>,
) -> io::Result<()> {
    record(patch, origin, origins)?;

    if patch.linting.presets.is_some() || patch.linting.selected.is_some() {
        for (identity, configuration) in &settings.linting.rules {
            let field = format!("linting.rules.{identity}.enabled");
            let value = serde_json::to_value(configuration.enabled).map_err(io::Error::other)?;
            collect(&value, &field, origin, origins, false);
        }
    }

    if let Some(warnings) = &patch.linting.warnings {
        for identity in warnings.keys() {
            if let Some(configuration) = settings.linting.rules.get(identity) {
                collect(
                    &serde_json::Value::Bool(configuration.enabled),
                    &format!("linting.rules.{identity}.enabled"),
                    origin,
                    origins,
                    false,
                );
            }
        }
    }

    if patch.linting.upstream_errors.is_some() {
        for definition in instar_lint::rules() {
            if matches!(definition.producer, RuleProducer::Upstream(_))
                && let Some(configuration) = settings.linting.rules.get(&definition.identity)
            {
                collect(
                    &serde_json::to_value(configuration.severity).map_err(io::Error::other)?,
                    &format!("linting.rules.{}.severity", definition.identity),
                    origin,
                    origins,
                    false,
                );
            }
        }
    }

    if patch.checking.type_errors.is_some() {
        collect(
            &serde_json::Value::Bool(settings.type_errors),
            "checking.type_errors",
            origin,
            origins,
            false,
        );
    }

    if patch.resolution.sourcemaps.is_some() {
        collect(
            &serde_json::Value::Bool(settings.resolution.sourcemaps_explicit),
            "resolution.sourcemaps_explicit",
            origin,
            origins,
            false,
        );
    }

    if patch.environment.roblox.is_some() {
        collect(
            &serde_json::Value::Bool(settings.environment.roblox_explicit),
            "environment.roblox_explicit",
            origin,
            origins,
            false,
        );
    }

    if patch.environment.globals.is_some() {
        collect(
            &serde_json::to_value(&settings.linting.globals).map_err(io::Error::other)?,
            "linting.globals",
            origin,
            origins,
            false,
        );
    }

    if patch.environment.roblox.is_some() {
        collect(
            &serde_json::Value::Bool(settings.linting.roblox),
            "linting.roblox",
            origin,
            origins,
            false,
        );
    }

    Ok(())
}

pub(crate) fn policy(
    policy: &DiagnosticPolicy,
    origin: &SettingOrigin,
    origins: &mut Vec<SettingOrigin>,
) -> io::Result<()> {
    let defaults = serde_json::to_value(DiagnosticPolicy::default()).map_err(io::Error::other)?;
    let value = serde_json::to_value(policy).map_err(io::Error::other)?;

    if let Some(fields) = value.as_object() {
        for (field, value) in fields {
            let precedence = if defaults.get(field) == Some(value) {
                0
            } else {
                origin.precedence
            };

            collect(
                value,
                &format!("diagnostics.{field}"),
                &SettingOrigin {
                    precedence,
                    ..origin.clone()
                },
                origins,
                false,
            );
        }
    }

    Ok(())
}

fn collect(
    value: &serde_json::Value,
    prefix: &str,
    origin: &SettingOrigin,
    origins: &mut Vec<SettingOrigin>,
    patch: bool,
) {
    if let Some(fields) = value.as_object() {
        for (field, value) in fields {
            let field = if prefix.is_empty() {
                field.clone()
            } else {
                format!("{prefix}.{field}")
            };

            collect(value, &field, origin, origins, patch);
        }
    } else {
        if patch
            && ((value.is_null() && !prefix.starts_with("resolution.aliases."))
                || (matches!(value, serde_json::Value::Bool(false))
                    && prefix.starts_with("selection.")
                    && prefix.ends_with(".override_exclusions")))
        {
            return;
        }

        origins.retain(|previous| {
            previous.field != prefix && !previous.field.starts_with(&format!("{prefix}."))
        });

        origins.push(SettingOrigin {
            field: prefix.to_owned(),
            ..origin.clone()
        });
    }
}
