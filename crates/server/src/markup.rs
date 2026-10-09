use instar_core::documentation::Entry;
use tower_lsp_server::gen_lsp_types::{Documentation, MarkupContent, MarkupKind};

pub(crate) fn documentation(entry: Entry, kind: MarkupKind) -> Documentation {
    let mut parts = Vec::new();

    if let Some(text) = entry.documentation {
        parts.push(text);
    }

    if let Some(sample) = entry.code_sample {
        parts.push(code(&sample, &kind));
    }

    if let Some(link) = entry.learn_more_link {
        parts.push(link);
    }

    MarkupContent {
        kind,
        value: parts.join("\n\n"),
    }
    .into()
}

pub(crate) fn code(text: &str, markup: &MarkupKind) -> String {
    if *markup != MarkupKind::Markdown {
        return text.to_owned();
    }

    let mut fence = "```".to_owned();

    while text.contains(&fence) {
        fence.push('`');
    }

    format!("{fence}luau\n{text}\n{fence}")
}
