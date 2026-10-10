use std::collections::{BTreeMap, VecDeque};

use instar_luau::SemanticIdentity;
use instar_project::{InputVersion, ProjectSnapshot, ValidatedEdit};
use instar_shared::SourceVersion;
use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{
        CallHierarchyItem, CodeAction, CodeLens, Command, CompletionItem, Diagnostic, DocumentLink,
        InlayHint, Location, SemanticToken, SemanticTokensLegend, TextEdit, TypeHierarchyItem,
        WorkspaceSymbol,
    },
    jsonrpc::{Error, Result},
};

const RETAINED_IDENTITIES: usize = 256;
const RETAINED_TOKENS: usize = 32;

#[derive(Clone)]
pub(crate) struct LazyRecord {
    pub(crate) source: Option<SourceVersion>,
    pub(crate) inputs: Vec<InputVersion>,
    pub(crate) snapshot: ProjectSnapshot,
    pub(crate) result: RetainedResult,
}

#[derive(Clone)]
pub(crate) enum RetainedResult {
    Completion {
        item: CompletionItem,
        documentation: Option<String>,
        edits: Vec<TextEdit>,
    },

    Hint {
        item: InlayHint,
        description: Option<String>,
        documentation: Option<String>,
    },

    Lens {
        item: CodeLens,
        command: Command,
    },

    Symbol {
        item: WorkspaceSymbol,
        location: Location,
    },

    Link(DocumentLink),
    Action(CodeAction),

    CallHierarchy {
        item: CallHierarchyItem,
        identity: SemanticIdentity,
    },

    TypeHierarchy {
        item: TypeHierarchyItem,
        identity: SemanticIdentity,
    },

    Command {
        command: String,
        edit: Option<ValidatedEdit>,
    },

    Diagnostics(Vec<Diagnostic>),
}

impl RetainedResult {
    fn operation(&self) -> &'static str {
        match self {
            Self::Completion { .. } => "completion",
            Self::Hint { .. } => "hint",
            Self::Lens { .. } => "lens",
            Self::Symbol { .. } => "symbol",
            Self::Link(_) => "link",
            Self::Action(_) => "action",
            Self::CallHierarchy { .. } => "calls",
            Self::TypeHierarchy { .. } => "types",
            Self::Command { .. } => "command",
            Self::Diagnostics(_) => "diagnostics",
        }
    }
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
        let data = data
            .and_then(Value::as_object)
            .ok_or_else(|| Error::invalid_params("invalid result identity"))?;

        if data.len() != 2 || !data.contains_key("session") || !data.contains_key("identity") {
            return Err(Error::invalid_params("invalid result identity"));
        }

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

        if record.result.operation() != operation {
            return Err(Error::invalid_params(
                "result identity has a different operation",
            ));
        }

        Ok(record.clone())
    }

    pub(crate) fn find(&self, record: &LazyRecord) -> Option<Value> {
        self.records
            .iter()
            .find(|(_, existing)| {
                existing.source == record.source
                    && existing.inputs == record.inputs
                    && matches!(
                        (&existing.result, &record.result),
                        (RetainedResult::Diagnostics(existing), RetainedResult::Diagnostics(current))
                            if existing == current
                    )
            })
            .map(|(identity, _)| json!({"session": self.session, "identity": identity}))
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
