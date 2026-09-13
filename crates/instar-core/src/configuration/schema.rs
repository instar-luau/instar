use super::InstarConfig;

impl InstarConfig {
    #[must_use]
    /// Generate the JSON schema for supported project configuration.
    pub fn schema() -> schemars::Schema {
        let mut schema = schemars::schema_for!(Self);

        if let Some(object) = schema.as_object_mut() {
            for value in object.values_mut() {
                describe(value);
            }
        }

        schema
    }
}

fn describe(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(default) = object.get("default") {
                let default = if default.is_null() {
                    "unset".to_owned()
                } else {
                    default.to_string()
                };

                if let Some(serde_json::Value::String(description)) = object.get_mut("description")
                {
                    description.push_str("\n\nDefault: `");
                    description.push_str(&default);
                    description.push_str("`.");
                }
            }

            for value in object.values_mut() {
                describe(value);
            }
        }

        serde_json::Value::Array(values) => {
            for value in values {
                describe(value);
            }
        }

        _ => {}
    }
}
