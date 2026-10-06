//! Native Luau configuration.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

/// An opaque configuration initialized and layered using upstream Luau APIs.
pub struct Configuration {
    native: cxx::UniquePtr<boundary::NativeConfiguration>,
}

impl fmt::Debug for Configuration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Configuration")
            .finish_non_exhaustive()
    }
}

impl Configuration {
    /// Creates a configuration with upstream defaults.
    ///
    /// # Errors
    /// Returns an error if native configuration allocation fails.
    pub fn new() -> io::Result<Self> {
        let native = boundary::create().map_err(io::Error::other)?;

        if native.is_null() {
            return Err(io::Error::other("native configuration allocation failed"));
        }

        Ok(Self { native })
    }

    /// Applies a `.luaurc` or `.config.luau` layer transactionally.
    ///
    /// Alias paths retain their defining file's directory. Luau code runs in
    /// the upstream sandbox without require, filesystem, or network access.
    /// The timeout is checked at VM safepoints and during extraction guards;
    /// it is not a hard limit on compilation, native calls, or allocation.
    /// Upstream uses an unrestricted allocator. Returned tables are limited
    /// to 128 levels and 100,000 expanded entries and cannot contain cycles.
    ///
    /// # Errors
    /// Returns a source-qualified error for unsupported paths, parsing,
    /// execution, extraction limits, or allocation. Failure leaves the
    /// previous configuration unchanged.
    pub fn apply(&mut self, source: &str, path: &Path, timeout: Duration) -> io::Result<()> {
        let executable = match path.file_name().and_then(|name| name.to_str()) {
            Some(".luaurc") => false,
            Some(".config.luau") => true,

            _ => {
                return Err(source_error(
                    path,
                    io::ErrorKind::InvalidInput,
                    "expected .luaurc or .config.luau",
                ));
            }
        };

        let absolute =
            std::path::absolute(path).map_err(|error| source_error(path, error.kind(), error))?;

        let location = absolute.to_str().ok_or_else(|| {
            source_error(
                path,
                io::ErrorKind::InvalidInput,
                "configuration path must be UTF-8",
            )
        })?;

        let outcome = self
            .native
            .pin_mut()
            .apply(source, location, executable, timeout.as_secs_f64())
            .map_err(|error| source_error(path, io::ErrorKind::Other, error))?;

        if outcome.message.is_empty() {
            return Ok(());
        }

        let kind = if outcome.timed_out {
            io::ErrorKind::TimedOut
        } else {
            io::ErrorKind::InvalidData
        };

        Err(source_error(path, kind, outcome.message))
    }

    /// Copies the effective native settings into safe Rust values.
    ///
    /// # Errors
    /// Returns an error if native snapshot allocation or conversion fails.
    pub fn snapshot(&self) -> io::Result<Snapshot> {
        let native = self
            .native
            .as_ref()
            .ok_or_else(|| io::Error::other("native configuration is unavailable"))?
            .snapshot()
            .map_err(io::Error::other)?;

        let mode = match native.mode.as_str() {
            "nocheck" => Mode::NoCheck,
            "nonstrict" => Mode::NonStrict,
            "strict" => Mode::Strict,
            "definition" => Mode::Definition,
            _ => return Err(io::Error::other("unknown upstream language mode")),
        };

        Ok(Snapshot {
            mode,
            lint: native
                .lint
                .into_iter()
                .map(|lint| {
                    (
                        lint.name,
                        LintPolicy {
                            enabled: lint.enabled,
                            fatal: lint.fatal,
                        },
                    )
                })
                .collect(),
            lint_errors: native.lint_errors,
            type_errors: native.type_errors,
            globals: native.globals,
            aliases: native
                .aliases
                .into_iter()
                .map(|alias| {
                    let directory = Path::new(&alias.location)
                        .parent()
                        .unwrap_or_else(|| Path::new(""));

                    (
                        alias.name,
                        Alias {
                            value: alias.value,
                            directory: directory.to_path_buf(),
                            original_case: alias.original_case,
                        },
                    )
                })
                .collect(),
        })
    }
}

/// Upstream Luau language mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Disables type inference.
    NoCheck,

    /// Treats unannotated symbols as any.
    NonStrict,

    /// Infers unannotated symbols.
    Strict,

    /// Uses upstream type-definition parsing rules.
    Definition,
}

/// The independent upstream lint masks for a named warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LintPolicy {
    /// Whether the warning is enabled.
    pub enabled: bool,

    /// Whether the warning is marked fatal.
    pub fatal: bool,
}

/// A native require alias and its defining directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alias {
    /// The upstream alias value, without resolving it against the directory.
    pub value: String,

    /// The directory containing the layer that defined this alias.
    pub directory: PathBuf,

    /// The alias spelling in its defining layer.
    pub original_case: String,
}

/// A detached snapshot of effective upstream native settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    /// The effective language mode.
    pub mode: Mode,

    /// Named warning policies, including disabled warnings.
    pub lint: BTreeMap<String, LintPolicy>,

    /// Whether upstream lint errors are enabled.
    pub lint_errors: bool,

    /// Whether upstream type errors are enabled.
    pub type_errors: bool,

    /// The effective upstream globals list.
    pub globals: Vec<String>,

    /// Aliases keyed by upstream's normalized alias names.
    pub aliases: BTreeMap<String, Alias>,
}

fn source_error(path: &Path, kind: io::ErrorKind, message: impl fmt::Display) -> io::Error {
    io::Error::new(kind, format!("{}: {message}", path.display()))
}

#[expect(
    unsafe_code,
    unreachable_pub,
    reason = "CXX generates public unsafe declarations inside this private native boundary"
)]
mod boundary {
    pub(super) use native::{NativeConfiguration, create};

    #[cxx::bridge(namespace = "instar")]
    mod native {
        struct NativeOutcome {
            message: String,
            timed_out: bool,
        }

        struct NativeLint {
            name: String,
            enabled: bool,
            fatal: bool,
        }

        struct NativeAlias {
            name: String,
            value: String,
            location: String,
            original_case: String,
        }

        struct NativeSnapshot {
            mode: String,
            lint: Vec<NativeLint>,
            lint_errors: bool,
            type_errors: bool,
            globals: Vec<String>,
            aliases: Vec<NativeAlias>,
        }

        unsafe extern "C++" {
            include!("src/configuration.hpp");

            type NativeConfiguration;

            fn create() -> Result<UniquePtr<NativeConfiguration>>;
            fn apply(
                self: Pin<&mut NativeConfiguration>,
                source: &str,
                path: &str,
                executable: bool,
                timeout_seconds: f64,
            ) -> Result<NativeOutcome>;
            fn snapshot(self: &NativeConfiguration) -> Result<NativeSnapshot>;
        }
    }
}
