use std::collections::{BTreeMap, VecDeque};

use instar_project::{InputVersion, ProjectSnapshot, ValidatedEdit};
use instar_shared::SourceVersion;
use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{SemanticToken, SemanticTokensLegend},
    jsonrpc::{Error, Result},
};

const RETAINED_IDENTITIES: usize = 256;
const RETAINED_TOKENS: usize = 32;

#[derive(Clone)]
pub(crate) struct LazyRecord {
    pub(crate) operation: &'static str,
    pub(crate) source: Option<SourceVersion>,
    pub(crate) inputs: Vec<InputVersion>,
    pub(crate) snapshot: ProjectSnapshot,
    pub(crate) payload: Value,
    pub(crate) edit: Option<ValidatedEdit>,
}

#[derive(Clone)]
pub(crate) struct TokenRecord {
    pub(crate) source: SourceVersion,
    pub(crate) legend: SemanticTokensLegend,
    pub(crate) tokens: Vec<SemanticToken>,
}

pub(crate) struct Identities {
    session: u64,
    next: u64,
    records: BTreeMap<u64, LazyRecord>,
    order: VecDeque<u64>,
    pub(crate) tokens: BTreeMap<String, TokenRecord>,
    token_order: VecDeque<String>,
}

impl Identities {
    pub(crate) fn new(session: u64) -> Self {
        Self {
            session,
            next: 0,
            records: BTreeMap::new(),
            order: VecDeque::new(),
            tokens: BTreeMap::new(),
            token_order: VecDeque::new(),
        }
    }

    pub(crate) fn insert(&mut self, record: LazyRecord) -> Result<Value> {
        self.next = self.next.checked_add(1).ok_or_else(Error::internal_error)?;
        let identity = self.next;
        self.records.insert(identity, record);
        self.order.push_back(identity);

        while self.order.len() > RETAINED_IDENTITIES {
            if let Some(identity) = self.order.pop_front() {
                self.records.remove(&identity);
            }
        }

        Ok(json!({"session": self.session, "identity": identity}))
    }

    pub(crate) fn get(&self, data: Option<&Value>, operation: &str) -> Result<LazyRecord> {
        let data = data.ok_or_else(|| Error::invalid_params("missing result identity"))?;

        if data.get("session").and_then(Value::as_u64) != Some(self.session) {
            return Err(Error::invalid_params("result belongs to another session"));
        }

        let identity = data
            .get("identity")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::invalid_params("invalid result identity"))?;

        let record = self
            .records
            .get(&identity)
            .ok_or_else(Error::content_modified)?;

        if record.operation != operation {
            return Err(Error::invalid_params(
                "result identity has a different operation",
            ));
        }

        Ok(record.clone())
    }

    pub(crate) fn store_tokens(&mut self, record: TokenRecord) -> Result<String> {
        self.next = self.next.checked_add(1).ok_or_else(Error::internal_error)?;
        let identity = format!("{}:{}", self.session, self.next);
        self.tokens.insert(identity.clone(), record);
        self.token_order.push_back(identity.clone());

        while self.token_order.len() > RETAINED_TOKENS {
            if let Some(identity) = self.token_order.pop_front() {
                self.tokens.remove(&identity);
            }
        }

        Ok(identity)
    }

    pub(crate) fn forget(&mut self, source: SourceVersion) {
        self.records.retain(|_, record| {
            record
                .source
                .is_none_or(|version| version.identity != source.identity)
        });

        self.tokens
            .retain(|_, record| record.source.identity != source.identity);

        self.order
            .retain(|identity| self.records.contains_key(identity));

        self.token_order
            .retain(|identity| self.tokens.contains_key(identity));
    }
}
