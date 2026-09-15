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
