use crate::analysis;

pub(super) fn render(documentation: &analysis::Documentation, symbol: &str) -> Option<String> {
    let value = documentation.get(symbol)?;

    if let Some(text) = value.as_str() {
        return (!text.is_empty()).then(|| text.to_owned());
    }

    let mut sections = Vec::new();

    for field in ["documentation", "learn_more_link", "code_sample"] {
        if let Some(text) = value
            .get(field)
            .and_then(|value| value.as_str())
            .filter(|text| !text.is_empty())
        {
            sections.push(match field {
                "learn_more_link" => format!("[Learn More]({text})"),
                "code_sample" => format!("```luau\n{text}\n```"),
                _ => text.to_owned(),
            });
        }
    }

    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

pub(super) fn comments(text: &str) -> String {
    let mut prose = Vec::new();
    let mut fields = Vec::new();
    let mut parameters = Vec::new();
    let mut returns = Vec::new();
    let mut errors = Vec::new();
    let mut fence = None;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = &trimmed[..3];

            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }

            prose.push(line.to_owned());
            continue;
        }

        if fence.is_some() {
            prose.push(line.to_owned());
            continue;
        }

        let (tag, value) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));

        let value = value.trim();

        match tag {
            "@param" => parameters.push(named(value)),
            "@field" => fields.push(named(value)),
            "@return" => returns.push(result(value)),
            "@error" | "@throws" => errors.push(result(value)),
            "@since" => prose.push(format!("**Since** `{value}`")),
            "@deprecated" => prose.push(format!("**Deprecated** {value}")),
            "@private" => prose.push("**Private**".into()),
            "@yields" => prose.push("**Yields**".into()),
            "@unreleased" => prose.push("**Unreleased**".into()),
            "@server" => prose.push("**Server**".into()),
            "@client" => prose.push("**Client**".into()),
            "@plugin" => prose.push("**Plugin**".into()),
            "@readonly" => prose.push("**Read Only**".into()),

            "@ignore" | "@tag" | "@within" | "@class" | "@function" | "@method" | "@prop"
            | "@interface" | "@type" | "@__index" | "@external" => {}

            _ => prose.push(line.to_owned()),
        }
    }

    let mut output = prose.join("\n").trim().to_owned();

    for (heading, entries) in [
        ("Fields", fields),
        ("Parameters", parameters),
        ("Returns", returns),
        ("Throws", errors),
    ] {
        if !entries.is_empty() {
            if !output.is_empty() {
                output.push_str("\n\n");
            }

            output.push_str("**");
            output.push_str(heading);
            output.push_str("**\n\n");
            output.push_str(&entries.join("\n"));
        }
    }

    output
}

fn named(value: &str) -> String {
    let (name, description) = value.split_once(char::is_whitespace).unwrap_or((value, ""));

    format!("- `{name}` {description}").trim_end().to_owned()
}

fn result(value: &str) -> String {
    value.split_once(" --").map_or_else(
        || format!("- {value}"),
        |(kind, description)| format!("- `{kind}` --{description}"),
    )
}

#[cfg(test)]
mod tests {
    use super::comments;

    #[test]
    fn moonwave_sections_preserve_examples() {
        let rendered = comments(
            "Adds values.\n@param amount number -- Amount\n@return number -- Total\n@error string -- Failure\n@within Counter\n@deprecated Use sum.\n```luau\n@param kept\n```",
        );

        assert!(rendered.contains("**Parameters**\n\n- `amount` number -- Amount"));
        assert!(rendered.contains("**Returns**\n\n- `number` -- Total"));
        assert!(rendered.contains("**Throws**\n\n- `string` -- Failure"));
        assert!(rendered.contains("**Deprecated** Use sum."));
        assert!(rendered.contains("```luau\n@param kept\n```"));
        assert!(!rendered.contains("@within"));
    }
}
