use std::{collections::BTreeMap, io, path::PathBuf, sync::Arc};

use instar_luau::{
    AnalysisRequest, AnalysisResult,
    flags::{FlagValue, LanguageProfile},
};

use instar_shared::{ContextIdentity, SourceVersion};

use crate::{
    configuration::{ConfigurationLayer, ProjectSettings},
    inputs::InputVersion,
    preparation::SyntaxEntry,
    selection::Matcher,
};

pub(crate) struct AnalysisEntry {
    pub(crate) recipe: AnalysisRequest,
    pub(crate) result: Arc<AnalysisResult>,
    pub(crate) inputs: Vec<InputVersion>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SettingsKey {
    pub(crate) context: ContextIdentity,
    pub(crate) directory: PathBuf,
    pub(crate) base: PathBuf,
    pub(crate) configuration: Option<PathBuf>,
    pub(crate) overrides: String,
}

pub(crate) struct SettingsEntry {
    pub(crate) settings: ProjectSettings,
    pub(crate) layers: Vec<Arc<ConfigurationLayer>>,
    pub(crate) inputs: Vec<InputVersion>,
    pub(crate) consulted: Vec<PathBuf>,
}

#[derive(Default)]
pub(crate) struct Cache {
    pub(crate) layers: BTreeMap<(SourceVersion, PathBuf, String), Arc<ConfigurationLayer>>,
    pub(crate) history: BTreeMap<PathBuf, Arc<ConfigurationLayer>>,
    pub(crate) settings: BTreeMap<SettingsKey, Arc<SettingsEntry>>,
    pub(crate) profiles: Vec<(BTreeMap<String, FlagValue>, LanguageProfile)>,
    pub(crate) matchers: BTreeMap<Vec<String>, Arc<Matcher>>,
    pub(crate) syntax: BTreeMap<SourceVersion, SyntaxEntry>,
    pub(crate) analyses: Vec<AnalysisEntry>,
}

impl Cache {
    pub(crate) fn matcher(&mut self, patterns: Option<&[String]>) -> io::Result<Arc<Matcher>> {
        let patterns = patterns.unwrap_or_default();

        if let Some(matcher) = self.matchers.get(patterns) {
            return Ok(Arc::clone(matcher));
        }

        let matcher = Arc::new(Matcher::new(Some(patterns))?);

        self.matchers
            .insert(patterns.to_vec(), Arc::clone(&matcher));

        Ok(matcher)
    }
}
