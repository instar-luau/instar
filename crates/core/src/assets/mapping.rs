use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use vermis::token::Symbol;

use super::{
    Error,
    syntax::{self, Output},
};

pub(super) const HIERARCHY: [&str; 5] = [
    "None",
    "LocalUserSecurity",
    "PluginSecurity",
    "WritePlayerSecurity",
    "RobloxScriptSecurity",
];

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Dump {
    pub(super) classes: Vec<Value>,

    #[serde(rename = "Enums")]
    pub(super) enumerations: Vec<Enumeration>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Types {
    pub(super) data_types: Vec<Value>,
    pub(super) constructors: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Corrections {
    pub(super) classes: Vec<Value>,
    pub(super) types: BTreeMap<String, String>,
    pub(super) exclusions: Exclusions,
    pub(super) exports: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Exclusions {
    pub(super) types: Vec<String>,
    pub(super) sections: Vec<String>,
    pub(super) globals: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Enumeration {
    pub(super) name: String,
    pub(super) items: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Item {
    pub(super) name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Class {
    pub(super) name: String,
    pub(super) members: Vec<Member>,
    pub(super) superclass: Option<String>,
    pub(super) tags: Option<Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Member {
    pub(super) name: String,

    #[serde(rename = "MemberType")]
    pub(super) kind: String,

    pub(super) value_type: Option<Type>,
    pub(super) return_type: Option<Value>,
    pub(super) tuple_returns: Option<Value>,
    pub(super) parameters: Option<Vec<Parameter>>,
    pub(super) security: Option<Value>,
    pub(super) tags: Option<Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Parameter {
    pub(super) name: String,

    #[serde(rename = "Type")]
    pub(super) value: Type,

    #[serde(flatten)]
    pub(super) fields: Map<String, Value>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Type {
    pub(super) generic: Option<String>,
    pub(super) union: Option<Vec<Self>>,
    pub(super) tuple: Option<Box<Self>>,
    pub(super) variadic: Option<Box<Self>>,
    pub(super) declared: Option<String>,
    pub(super) name: Option<String>,
    pub(super) category: Option<String>,
}

impl Type {
    pub(super) fn resolve(&self, mappings: &BTreeMap<String, String>) -> Result<String, Error> {
        let mut output = Output::default();
        self.emit(mappings, &mut output)?;

        output.finish(|| false)
    }

    fn emit(&self, mappings: &BTreeMap<String, String>, output: &mut Output) -> Result<(), Error> {
        if let Some(generic) = &self.generic {
            output.symbol(Symbol::LeftBrace);
            output.source(generic);
            output.symbol(Symbol::RightBrace);

            return Ok(());
        }

        if let Some(union) = &self.union {
            for (index, part) in union.iter().enumerate() {
                if index > 0 {
                    output.symbol(Symbol::Pipe);
                }

                part.emit(mappings, output)?;
            }

            return Ok(());
        }

        if let Some(variadic) = self.tuple.as_ref().or(self.variadic.as_ref()) {
            output.symbol(Symbol::Ellipsis);

            return variadic.emit(mappings, output);
        }

        let name = self
            .declared
            .as_ref()
            .or(self.name.as_ref())
            .ok_or_else(|| invalid("missing type name"))?;

        if let Some(name) = name.strip_suffix('?') {
            let value = Self {
                declared: None,
                name: Some(name.to_owned()),
                ..self.clone()
            };

            value.emit(mappings, output)?;
            output.symbol(Symbol::QuestionMark);
        } else if self.category.as_deref() == Some("Enum") && !name.starts_with("Enum.") {
            output.source(format!("Enum.{name}"));
        } else {
            output.source(mappings.get(name).unwrap_or(name));
        }

        Ok(())
    }

    pub(super) fn references(
        &self,
        mappings: &BTreeMap<String, String>,
        result: &mut BTreeSet<String>,
    ) -> Result<(), Error> {
        if let Some(union) = &self.union {
            for part in union {
                part.references(mappings, result)?;
            }

            return Ok(());
        }

        if let Some(nested) = self.tuple.as_ref().or(self.variadic.as_ref()) {
            return nested.references(mappings, result);
        }

        let value = if let Some(generic) = &self.generic {
            let value = Self {
                name: Some(generic.clone()),
                ..Self::default()
            };

            value.resolve(mappings)?
        } else {
            self.resolve(mappings)?
        };

        let value = value.strip_suffix('?').unwrap_or(&value);

        if (syntax::identifier(value)
            || value.strip_prefix("Enum.").is_some_and(syntax::identifier))
            && !primitive(value)
        {
            result.insert(value.to_owned());
        }

        Ok(())
    }
}

impl Member {
    pub(super) fn access(&self, labels: &[&str]) -> Result<bool, Error> {
        if tagged(self.tags.as_deref(), "NotScriptable") {
            return Ok(false);
        }

        match self.security.as_ref() {
            Some(Value::Object(_)) => {
                let (read, write) = self.property_security()?;

                Ok(read.is_some_and(|label| labels.contains(&label))
                    || write.is_some_and(|label| labels.contains(&label)))
            }

            Some(Value::String(label)) => Ok(labels.contains(&label.as_str())),
            None => Ok(labels.contains(&"None")),
            Some(_) => Ok(false),
        }
    }

    pub(super) fn permissions(&self, labels: &[&str]) -> Result<(bool, bool), Error> {
        let (read, write) = self.property_security()?;

        let read = read.is_some_and(|label| labels.contains(&label))
            && !tagged(self.tags.as_deref(), "WriteOnly");

        let write = write.is_some_and(|label| labels.contains(&label))
            && !tagged(self.tags.as_deref(), "ReadOnly");

        Ok((read, write))
    }

    pub(super) fn parameters(&self) -> Result<&[Parameter], Error> {
        self.parameters
            .as_deref()
            .ok_or_else(|| invalid(format!("missing parameters for `{}`", self.name)))
    }

    pub(super) fn returns(&self) -> Result<Vec<Type>, Error> {
        if let Some(value) = self.tuple_returns.as_ref().or(self.return_type.as_ref()) {
            match value {
                Value::Array(values) => values
                    .iter()
                    .cloned()
                    .map(|value| decode(value, "tuple returns"))
                    .collect(),

                value => Ok(vec![decode(value.clone(), "tuple returns")?]),
            }
        } else {
            Ok(Vec::new())
        }
    }

    fn property_security(&self) -> Result<(Option<&str>, Option<&str>), Error> {
        match &self.security {
            Some(Value::Object(security)) => {
                let read = security
                    .get("Read")
                    .ok_or_else(|| invalid(format!("missing Read security for `{}`", self.name)))?
                    .as_str();

                let write = security
                    .get("Write")
                    .ok_or_else(|| invalid(format!("missing Write security for `{}`", self.name)))?
                    .as_str();

                Ok((read, write))
            }

            None => Ok((Some("None"), Some("None"))),

            Some(_) => Err(invalid(format!(
                "invalid property security for `{}`",
                self.name
            ))),
        }
    }
}

pub(super) fn decode<Target: DeserializeOwned>(value: Value, input: &str) -> Result<Target, Error> {
    serde_json::from_value(value).map_err(|source| Error::Data {
        input: input.to_owned(),
        source,
    })
}

pub(super) fn parse<Target: DeserializeOwned>(source: &str, input: &str) -> Result<Target, Error> {
    serde_json::from_str(source).map_err(|source| Error::Data {
        input: input.to_owned(),
        source,
    })
}

pub(super) fn invalid(reason: impl Into<String>) -> Error {
    Error::Invalid {
        reason: reason.into(),
    }
}

pub(super) fn primitive(name: &str) -> bool {
    matches!(
        name,
        "string"
            | "number"
            | "boolean"
            | "nil"
            | "any"
            | "unknown"
            | "never"
            | "buffer"
            | "thread"
            | "vector"
    )
}

pub(super) fn tagged(tags: Option<&[Value]>, name: &str) -> bool {
    tags.into_iter()
        .flatten()
        .any(|tag| tag.as_str() == Some(name))
}

pub(super) fn corrected(
    values: Vec<Value>,
    patches: &[Value],
    cancelled: fn() -> bool,
) -> Result<Vec<Class>, Error> {
    let mut result = Vec::with_capacity(values.len());

    for mut class in values {
        if cancelled() {
            return Err(Error::Cancelled);
        }

        let patch = patches
            .iter()
            .find(|patch| patch.get("Name") == class.get("Name"));

        if let Some(members) = class.get_mut("Members").and_then(Value::as_array_mut) {
            for member in members {
                if cancelled() {
                    return Err(Error::Cancelled);
                }

                if let Some(patch) = patch
                    .and_then(|patch| patch.get("Members"))
                    .and_then(Value::as_array)
                    .and_then(|patches| {
                        patches
                            .iter()
                            .find(|patch| patch.get("Name") == member.get("Name"))
                    })
                {
                    repair(member, patch)?;
                }
            }
        }

        let identity = class
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>")
            .to_owned();

        result.push(decode(class, &format!("API class {identity}"))?);
    }

    Ok(result)
}

fn repair(member: &mut Value, correction: &Value) -> Result<(), Error> {
    let member = member
        .as_object_mut()
        .ok_or_else(|| invalid("invalid API member"))?;

    for field in ["ValueType", "ReturnType"] {
        if let Some(patch) = correction.get(field).filter(|value| !value.is_null()) {
            merge(
                member
                    .entry(field)
                    .or_insert_with(|| Value::Object(Map::new())),
                patch,
            )?;
        }
    }

    if let Some(patch) = correction
        .get("TupleReturns")
        .filter(|value| !value.is_null())
    {
        member.insert("TupleReturns".to_owned(), patch.clone());
    }

    if let Some(patches) = correction.get("Parameters").and_then(Value::as_array) {
        let parameters = member
            .get_mut("Parameters")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| invalid("missing corrected parameters"))?;

        for parameter in parameters {
            if let Some(patch) = patches
                .iter()
                .find(|patch| patch.get("Name") == parameter.get("Name"))
            {
                let mut fields = patch
                    .as_object()
                    .ok_or_else(|| invalid("invalid parameter correction"))?
                    .clone();

                let value = fields.remove("Type");
                merge(parameter, &Value::Object(fields))?;

                if let Some(value) = value.filter(|value| !value.is_null()) {
                    let original = parameter
                        .get_mut("Type")
                        .ok_or_else(|| invalid("missing corrected parameter type"))?;

                    merge(original, &value)?;
                }
            }
        }
    }

    Ok(())
}

fn merge(value: &mut Value, patch: &Value) -> Result<(), Error> {
    if value.is_null() {
        *value = Value::Object(Map::new());
    }

    let value = value
        .as_object_mut()
        .ok_or_else(|| invalid("invalid corrected type"))?;

    let patch = patch
        .as_object()
        .ok_or_else(|| invalid("invalid type correction"))?;

    value.extend(patch.clone());

    Ok(())
}
