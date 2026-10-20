use std::{
    cell::OnceCell,
    collections::{BTreeMap, BTreeSet},
};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use vermis::token::Symbol;

use super::{Error, syntax::Output};
use crate::syntax::is_identifier;

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

    #[serde(skip)]
    decoded_returns: OnceCell<Vec<Type>>,

    #[serde(skip)]
    signature: OnceCell<Signature>,

    #[serde(skip)]
    security_facts: OnceCell<Security>,
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

pub(super) enum Signature {
    Property {
        value: String,
    },

    Function {
        parameters: Vec<Argument>,
        returns: Vec<String>,
    },

    Callback {
        parameters: Vec<Argument>,
        returns: Vec<String>,
    },

    Event {
        arguments: Vec<String>,
    },
}

pub(super) struct Argument {
    pub(super) name: String,
    pub(super) value: String,
    pub(super) has_default: bool,
}

enum Security {
    Label(String),

    Property {
        read: Option<String>,
        write: Option<String>,
        is_read_only: bool,
        is_write_only: bool,
    },

    Denied,
}

impl Type {
    pub(super) fn resolve(&self, mappings: &BTreeMap<String, String>) -> Result<String, Error> {
        let mut output = Output::default();
        self.emit(mappings, &mut output)?;

        output.finish(|| false)
    }

    fn emit(&self, mappings: &BTreeMap<String, String>, output: &mut Output) -> Result<(), Error> {
        self.validate_shape()?;

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
            .ok_or_else(|| invalid("Missing type name."))?;

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
        self.validate_shape()?;

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

        if (is_identifier(value) || value.strip_prefix("Enum.").is_some_and(is_identifier))
            && !is_primitive(value)
        {
            result.insert(value.to_owned());
        }

        Ok(())
    }

    fn validate_shape(&self) -> Result<(), Error> {
        if self.generic.is_some()
            && (self.union.is_some() || self.tuple.is_some() || self.variadic.is_some())
        {
            return Err(invalid("Generic and structural type shapes conflict."));
        }

        Ok(())
    }
}

impl Member {
    pub(super) fn can_access(&self, labels: &[&str]) -> Result<bool, Error> {
        let is_allowed = match self.security()? {
            Security::Label(label) => labels.contains(&label.as_str()),

            Security::Property { read, write, .. } => {
                read.as_deref().is_some_and(|label| labels.contains(&label))
                    || write
                        .as_deref()
                        .is_some_and(|label| labels.contains(&label))
            }

            Security::Denied => false,
        };

        Ok(is_allowed)
    }

    pub(super) fn permissions(&self, labels: &[&str]) -> Result<(bool, bool), Error> {
        let Security::Property {
            read,
            write,
            is_read_only,
            is_write_only,
        } = self.security()?
        else {
            return Err(invalid(format!(
                "Invalid property security for `{}`.",
                self.name
            )));
        };

        let can_read =
            read.as_deref().is_some_and(|label| labels.contains(&label)) && !*is_write_only;

        let can_write = write
            .as_deref()
            .is_some_and(|label| labels.contains(&label))
            && !*is_read_only;

        Ok((can_read, can_write))
    }

    pub(super) fn signature(
        &self,
        mappings: &BTreeMap<String, String>,
    ) -> Result<&Signature, Error> {
        if let Some(signature) = self.signature.get() {
            return Ok(signature);
        }

        let signature = match self.kind.as_str() {
            "Property" => Signature::Property {
                value: self
                    .value_type
                    .as_ref()
                    .ok_or_else(|| invalid("Missing property value type."))?
                    .resolve(mappings)?,
            },

            "Function" | "Callback" => {
                let parameters = self
                    .parameters()?
                    .iter()
                    .map(|parameter| {
                        Ok(Argument {
                            name: parameter.name.clone(),
                            value: parameter.value.resolve(mappings)?,
                            has_default: parameter.fields.contains_key("Default"),
                        })
                    })
                    .collect::<Result<_, Error>>()?;

                let returns = self
                    .returns("tuple returns")?
                    .iter()
                    .map(|value| value.resolve(mappings))
                    .collect::<Result<_, _>>()?;

                if self.kind == "Function" {
                    Signature::Function {
                        parameters,
                        returns,
                    }
                } else {
                    Signature::Callback {
                        parameters,
                        returns,
                    }
                }
            }

            "Event" => Signature::Event {
                arguments: self
                    .parameters()?
                    .iter()
                    .map(|parameter| parameter.value.resolve(mappings))
                    .collect::<Result<_, _>>()?,
            },

            kind => return Err(invalid(format!("Unsupported member kind `{kind}`."))),
        };

        Ok(self.signature.get_or_init(|| signature))
    }

    fn parameters(&self) -> Result<&[Parameter], Error> {
        self.parameters
            .as_deref()
            .ok_or_else(|| invalid(format!("Missing parameters for `{}`.", self.name)))
    }

    pub(super) fn returns(&self, input: &str) -> Result<&[Type], Error> {
        if let Some(values) = self.decoded_returns.get() {
            return Ok(values);
        }

        let values = if let Some(value) = self.tuple_returns.as_ref().or(self.return_type.as_ref())
        {
            match value {
                Value::Array(values) => values
                    .iter()
                    .cloned()
                    .map(|value| decode(value, input))
                    .collect::<Result<_, _>>()?,

                value => vec![decode(value.clone(), input)?],
            }
        } else {
            Vec::new()
        };

        Ok(self.decoded_returns.get_or_init(|| values))
    }

    fn security(&self) -> Result<&Security, Error> {
        if let Some(security) = self.security_facts.get() {
            return Ok(security);
        }

        let security = if has_tag(self.tags.as_deref(), "NotScriptable") {
            Security::Denied
        } else {
            match &self.security {
                Some(Value::Object(security)) => {
                    let read = security
                        .get("Read")
                        .ok_or_else(|| {
                            invalid(format!("Missing `Read` security for `{}`.", self.name))
                        })?
                        .as_str()
                        .map(str::to_owned);

                    let write = security
                        .get("Write")
                        .ok_or_else(|| {
                            invalid(format!("Missing `Write` security for `{}`.", self.name))
                        })?
                        .as_str()
                        .map(str::to_owned);

                    Security::Property {
                        read,
                        write,
                        is_read_only: has_tag(self.tags.as_deref(), "ReadOnly"),
                        is_write_only: has_tag(self.tags.as_deref(), "WriteOnly"),
                    }
                }

                None => Security::Property {
                    read: Some("None".to_owned()),
                    write: Some("None".to_owned()),
                    is_read_only: has_tag(self.tags.as_deref(), "ReadOnly"),
                    is_write_only: has_tag(self.tags.as_deref(), "WriteOnly"),
                },

                Some(Value::String(label)) => Security::Label(label.clone()),
                Some(_) => Security::Denied,
            }
        };

        Ok(self.security_facts.get_or_init(|| security))
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

pub(super) fn is_primitive(name: &str) -> bool {
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

pub(super) fn has_tag(tags: Option<&[Value]>, name: &str) -> bool {
    tags.into_iter()
        .flatten()
        .any(|tag| tag.as_str() == Some(name))
}

pub(super) fn corrected(
    values: Vec<Value>,
    patches: &[Value],
    is_cancelled: fn() -> bool,
) -> Result<Vec<Class>, Error> {
    let mut result = Vec::with_capacity(values.len());

    for mut class in values {
        if is_cancelled() {
            return Err(Error::Cancelled);
        }

        let patch = patches
            .iter()
            .find(|patch| patch.get("Name") == class.get("Name"));

        if let Some(members) = class.get_mut("Members").and_then(Value::as_array_mut) {
            for member in members {
                if is_cancelled() {
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
        .ok_or_else(|| invalid("API member must be an object."))?;

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
            .ok_or_else(|| invalid("Corrected member must have a `Parameters` array."))?;

        for parameter in parameters {
            if let Some(patch) = patches
                .iter()
                .find(|patch| patch.get("Name") == parameter.get("Name"))
            {
                let mut fields = patch
                    .as_object()
                    .ok_or_else(|| invalid("Parameter correction must be an object."))?
                    .clone();

                let value = fields.remove("Type");
                merge(parameter, &Value::Object(fields))?;

                if let Some(value) = value.filter(|value| !value.is_null()) {
                    let original = parameter
                        .get_mut("Type")
                        .ok_or_else(|| invalid("Corrected parameter is missing `Type`."))?;

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
        .ok_or_else(|| invalid("Corrected type must be an object."))?;

    let patch = patch
        .as_object()
        .ok_or_else(|| invalid("Type correction must be an object."))?;

    value.extend(patch.clone());

    Ok(())
}
