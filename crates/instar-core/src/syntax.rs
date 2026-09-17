use crate::source::Source;
use serde_json::Value;

pub(crate) fn kind(value: &Value) -> &str {
    value["type"].as_str().unwrap_or_default()
}

pub(crate) fn field<'value>(value: &'value Value, name: &str) -> &'value str {
    value[name].as_str().unwrap_or_default()
}

pub(crate) fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

pub(crate) fn decode(source: &Source, text: &str) -> Result<Value, serde_json::Error> {
    let normalized = normalize(text);

    match serde_json::from_str(&normalized) {
        Ok(value) => Ok(value),

        Err(error)
            if source
                .parse()
                .nodes
                .iter()
                .any(|node| node.kind == vermis::Kind::Instantiate) =>
        {
            let repaired = repair_instantiations(&normalized);

            serde_json::from_str(&repaired).or(Err(error))
        }

        Err(error) => Err(error),
    }
}

fn normalize(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut quoted = false;
    let mut escaped = false;
    let mut index = 0;

    while index < text.len() {
        let tail = &text[index..];

        if !quoted
            && let Some(number) = ["-Infinity", "Infinity", "NaN"]
                .into_iter()
                .find(|number| tail.starts_with(number))
        {
            output.push('"');
            output.push_str(number);
            output.push('"');
            index += number.len();
            continue;
        }

        let character = tail.chars().next().expect("remaining character");
        output.push(character);

        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        }

        index += character.len_utf8();
    }

    output
}

fn repair_instantiations(text: &str) -> String {
    let mut output = text.to_owned();

    while let Some(candidate) = candidates(&output)
        .into_iter()
        .min_by_key(|candidate| candidate.type_end - candidate.type_start)
    {
        let expression = &output[candidate.expression_start..candidate.expression_end];

        let arguments = candidate
            .types
            .iter()
            .map(|(start, end)| &output[*start..*end])
            .collect::<Vec<_>>();

        let location = serde_json::from_str::<Value>(expression)
            .ok()
            .and_then(|value| value["location"].as_str().map(str::to_owned));

        let end_location = arguments
            .last()
            .and_then(|argument| serde_json::from_str::<Value>(argument).ok())
            .and_then(|value| value["location"].as_str().map(str::to_owned));

        let location = match (location, end_location) {
            (Some(start), Some(end)) => {
                let start = start
                    .split_once(" - ")
                    .map_or(start.as_str(), |parts| parts.0);

                let end = end.split_once(" - ").map_or(end.as_str(), |parts| parts.1);

                format!("{start} - {end}")
            }

            (Some(location), _) => location,
            _ => String::new(),
        };

        let location = serde_json::to_string(&location).unwrap_or_else(|_| "\"\"".into());

        let mut repaired = format!(
            r#"{{"type":"AstExprInstantiate","location":{location},"expr":{expression},"typeArguments":["#
        );

        repaired.push_str(&arguments.join(","));
        repaired.push_str("]}");
        output.replace_range(candidate.expression_start..candidate.type_end, &repaired);
    }

    output
}

#[derive(Clone)]
struct Candidate {
    expression_start: usize,
    expression_end: usize,
    type_start: usize,
    type_end: usize,
    types: Vec<(usize, usize)>,
}

fn candidates(text: &str) -> Vec<Candidate> {
    let objects = object_spans(text);
    let mut result = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let bytes = text.as_bytes();

    for index in 0..bytes.len().saturating_sub(1) {
        let character = bytes[index] as char;

        if escaped {
            escaped = false;
            continue;
        }

        if quoted && character == '\\' {
            escaped = true;
            continue;
        }

        if character == '"' {
            quoted = !quoted;
            continue;
        }

        if quoted || bytes[index] != b'}' || bytes[index + 1] != b'{' {
            continue;
        }

        let Some(&(expression_start, expression_end)) =
            objects.iter().find(|(_, end)| *end == index + 1)
        else {
            continue;
        };

        if !text[expression_start..].starts_with(r#"{"type":"AstExpr"#) {
            continue;
        }

        let type_start = index + 1;

        let Some(&(_, first_type_end)) = objects.iter().find(|(start, _)| {
            *start == type_start && text[*start..].starts_with(r#"{"type":"AstType"#)
        }) else {
            continue;
        };

        let mut types = vec![(type_start, first_type_end)];
        let mut type_end = first_type_end;

        loop {
            let Some(&(next_start, next_end)) =
                objects.iter().find(|(start, _)| *start == type_end)
            else {
                break;
            };

            if !text[next_start..].starts_with(r#"{"type":"AstType"#) {
                break;
            }

            types.push((next_start, next_end));
            type_end = next_end;
        }

        result.push(Candidate {
            expression_start,
            expression_end,
            type_start,
            type_end,
            types,
        });
    }

    result
}

fn object_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut stack = Vec::new();
    let mut quoted = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }

        if quoted && byte == b'\\' {
            escaped = true;
            continue;
        }

        if byte == b'"' {
            quoted = !quoted;
            continue;
        }

        if quoted {
            continue;
        }

        match byte {
            b'{' => stack.push(index),

            b'}' => {
                if let Some(start) = stack.pop() {
                    result.push((start, index + 1));
                }
            }

            _ => {}
        }
    }

    result
}
