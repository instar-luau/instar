use crate::{Callbacks, Checker, bridge::native};

use native::{
    Completion, Hover, Navigation, Reference, ReferenceTarget, RenameTarget, SignatureHelp,
    TypeHint,
};

use std::io;

fn one<T>(mut output: Vec<T>, message: &str) -> io::Result<Option<T>> {
    if output.len() > 1 {
        return Err(io::Error::other(message));
    }

    Ok(output.pop())
}

impl Checker {
    /// Returns hover information at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn hover(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Option<Hover>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_hover(checker, host, path, [line, column], &mut output).into_result()?;

            one(output, "native hover returned multiple results")
        })
    }

    /// Returns completion items at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn completion(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Completion>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_completion(checker, host, path, [line, column], &mut output)
                .into_result()?;

            Ok(output)
        })
    }

    /// Returns signature help at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn signature_help(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Option<SignatureHelp>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_signature_help(checker, host, path, [line, column], &mut output)
                .into_result()?;

            one(output, "native signature help returned multiple results")
        })
    }

    /// Returns inferred type hints for a module.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn type_hints(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
    ) -> io::Result<Vec<TypeHint>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_type_hints(checker, host, path, &mut output).into_result()?;

            Ok(output)
        })
    }

    /// Returns definition targets at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn definition(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Navigation>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_definition(checker, host, path, [line, column], &mut output)
                .into_result()?;

            Ok(output)
        })
    }

    /// Returns declaration targets at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn declaration(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Navigation>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_declaration(checker, host, path, [line, column], &mut output)
                .into_result()?;

            Ok(output)
        })
    }

    /// Returns concrete source implementation targets at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn implementation(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Navigation>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_implementation(checker, host, path, [line, column], &mut output)
                .into_result()?;

            Ok(output)
        })
    }

    /// Returns type-definition targets at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn type_definition(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Navigation>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_type_definition(checker, host, path, [line, column], &mut output)
                .into_result()?;

            Ok(output)
        })
    }

    /// Returns reference occurrences at a source position.
    ///
    /// `candidates` restricts the native occurrence walk to exact module identities;
    /// `None` leaves it unrestricted. The selected source module must be included.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn references(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
        candidates: Option<&[String]>,
    ) -> io::Result<Vec<Reference>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_references(
                checker,
                host,
                path,
                [line, column],
                native::Candidates {
                    restricted: candidates.is_some(),
                    modules: candidates.unwrap_or_default(),
                },
                &mut output,
            )
            .into_result()?;

            Ok(output)
        })
    }

    /// Resolves the semantic symbol at a source position for candidate selection.
    ///
    /// # Errors
    /// Returns an error when native symbol resolution fails.
    pub fn reference_target(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Option<ReferenceTarget>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_reference_target(checker, host, path, [line, column], &mut output)
                .into_result()?;

            one(
                output,
                "native resolution returned multiple reference targets",
            )
        })
    }

    /// Resolves the source-editable target shared by prepare and rename.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn rename_target(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Option<RenameTarget>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_rename_target(checker, host, path, [line, column], &mut output)
                .into_result()?;

            one(output, "native resolution returned multiple rename targets")
        })
    }

    /// Validates a rename and returns every affected reference.
    ///
    /// `candidates` restricts the native occurrence walk to exact module identities;
    /// `None` leaves it unrestricted. The selected source module must be included.
    ///
    /// # Errors
    /// Returns an error for invalid names, conflicts, or incomplete native analysis.
    pub fn rename(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
        new_name: &str,
        candidates: Option<&[String]>,
    ) -> io::Result<Vec<Reference>> {
        self.with_callbacks(callbacks, |checker, host| {
            let mut output = Vec::new();

            native::editor_rename(
                checker,
                host,
                path,
                [line, column],
                new_name,
                native::Candidates {
                    restricted: candidates.is_some(),
                    modules: candidates.unwrap_or_default(),
                },
                &mut output,
            )
            .into_result()?;

            Ok(output)
        })
    }
}
