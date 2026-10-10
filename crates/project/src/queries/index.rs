use std::collections::BTreeMap;

use instar_luau::{
    AnalysisIdentity, FactsResult, OccurrenceFact, SemanticCategory, SemanticIdentity,
};

use instar_shared::{Availability, Knowledge, SourceVersion};

#[derive(Default)]
pub(crate) struct Index {
    pub(crate) analyses: BTreeMap<AnalysisIdentity, FactsResult>,
}

impl Index {
    pub(crate) fn replace(&mut self, facts: FactsResult) {
        self.analyses
            .retain(|identity, _| identity.module != facts.analysis.module);

        self.analyses.insert(facts.analysis, facts);
    }

    pub(crate) fn available(&self, categories: &[SemanticCategory]) -> bool {
        !self.analyses.is_empty()
            && self.analyses.values().all(|facts| {
                categories.iter().all(|category| {
                    facts.availability.categories.get(category) == Some(&Availability::Available)
                })
            })
    }

    pub(crate) fn selected(&self, source: SourceVersion, offset: usize) -> Option<&OccurrenceFact> {
        self.analyses
            .values()
            .flat_map(|facts| &facts.facts.occurrences)
            .find(|occurrence| {
                occurrence.location.source == source
                    && occurrence.location.span.start <= offset
                    && offset < occurrence.location.span.end
            })
            .or_else(|| {
                self.analyses
                    .values()
                    .flat_map(|facts| &facts.facts.occurrences)
                    .find(|occurrence| {
                        occurrence.location.source == source
                            && occurrence.location.span.end == offset
                    })
            })
    }

    pub(crate) fn targets(
        &self,
        source: SourceVersion,
        offset: usize,
    ) -> Option<&[SemanticIdentity]> {
        match &self.selected(source, offset)?.targets {
            Knowledge::Known(targets) => Some(targets),
            Knowledge::Unavailable(_) => None,
        }
    }
}
