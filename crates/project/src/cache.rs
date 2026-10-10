use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use instar_luau::{AnalysisRequest, AnalysisResult};
use instar_shared::SourceVersion;

use crate::{configuration::ConfigurationLayer, inputs::InputVersion};

pub(crate) struct AnalysisEntry {
    pub(crate) recipe: AnalysisRequest,
    pub(crate) result: Arc<AnalysisResult>,
    pub(crate) inputs: Vec<InputVersion>,
}

#[derive(Default)]
pub(crate) struct Cache {
    pub(crate) layers: BTreeMap<(SourceVersion, PathBuf, String), Arc<ConfigurationLayer>>,
    pub(crate) history: BTreeMap<PathBuf, Arc<ConfigurationLayer>>,
    pub(crate) analyses: Vec<AnalysisEntry>,
}
