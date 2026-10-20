//! Owned Luau type and pack snapshots, validated at the native boundary.

use std::mem::size_of;

use crate::{
    bridge::bindings::{
        self, NativeApplicationState, NativePackKind, NativePrimitiveState, NativeProvenance,
        NativeSingletonState, NativeTableState, NativeTypeKind, NativeUnresolvedState,
    },
    error::Error,
};

/// An index in a snapshot's type collection.
pub type TypeIndex = usize;

/// An index in a snapshot's pack collection.
pub type PackIndex = usize;

/// A snapshot-local semantic subject.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Subject {
    /// A type.
    Type(TypeIndex),

    /// A type pack.
    Pack(PackIndex),
}

/// An owned, bounded native semantic graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    /// Indexed type nodes.
    pub types: Vec<Type>,

    /// Indexed pack nodes.
    pub packs: Vec<Pack>,

    /// Selected semantic subject.
    pub root: Subject,
}

/// A Luau primitive or universal type.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Primitive {
    /// Nil.
    Nil,

    /// Boolean.
    Boolean,

    /// Number.
    Number,

    /// Integer.
    Integer,

    /// String.
    String,

    /// Thread.
    Thread,

    /// Function.
    Function,

    /// Table.
    Table,

    /// Buffer.
    Buffer,

    /// Any.
    Any,

    /// Unknown.
    Unknown,

    /// Never.
    Never,
}

/// An exact singleton value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Singleton {
    /// A boolean value.
    Boolean(bool),

    /// Original string bytes, including non-UTF-8 bytes.
    String(Vec<u8>),
}

/// The provenance of a native name.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Provenance {
    /// A binding verified against declaration identity and scope.
    Alias,

    /// An external nominal type.
    External,

    /// A compiler-authored display name.
    Synthetic,

    /// A name not verified as a source binding.
    Unverified,
}

/// A named type with optional bounded structural expansion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Named {
    /// Unqualified spelling.
    pub name: String,

    /// Verified qualification when available.
    pub qualifier: Option<String>,

    /// Name provenance.
    pub provenance: Provenance,

    /// Type instantiation arguments.
    pub type_arguments: Vec<TypeIndex>,

    /// Pack instantiation arguments.
    pub pack_arguments: Vec<PackIndex>,

    /// Available structural expansion.
    pub expansion: Option<TypeIndex>,
}

/// A generic's presentation-local identity and explicit spelling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Generic {
    /// Snapshot-local identity, never a solver index.
    pub identity: usize,

    /// Explicit source name when supplied.
    pub name: Option<String>,
}

/// An owned function type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Function {
    /// Type binders.
    pub generics: Vec<TypeIndex>,

    /// Pack binders.
    pub generic_packs: Vec<PackIndex>,

    /// Argument pack.
    pub arguments: PackIndex,

    /// Return pack.
    pub returns: PackIndex,

    /// Names indexed by original native argument position.
    pub argument_names: Vec<Option<String>>,

    /// Whether the checked attribute is present.
    pub checked: bool,
}

/// Native table state.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TableState {
    /// Inexact sealed table.
    Sealed,

    /// A directly constructed table.
    Unsealed,

    /// A free table.
    Free,

    /// A generic table.
    Generic,

    /// An exact table.
    Exact,
}

/// A property retaining separate access types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeProperty {
    /// Original property-name bytes.
    pub name: Vec<u8>,

    /// Read type, absent for write-only properties.
    pub read: Option<TypeIndex>,

    /// Write type, absent for read-only properties.
    pub write: Option<TypeIndex>,

    /// Whether native access types are shared.
    pub shared: bool,
}

/// A native table indexer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Indexer {
    /// Key type.
    pub key: TypeIndex,

    /// Value type.
    pub value: TypeIndex,

    /// Whether writes are prohibited.
    pub readonly: bool,
}

/// An owned table structure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Table {
    /// Deterministically ordered properties.
    pub properties: Vec<TypeProperty>,

    /// Optional indexer.
    pub indexer: Option<Indexer>,

    /// Native table state.
    pub state: TableState,
}

/// Why a type application remains unexpanded.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ApplicationState {
    /// A pending alias expansion.
    Pending,

    /// An unreduced type function.
    Unreduced,
}

/// An unapplied or unreduced native type operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Application {
    /// Available operation spelling.
    pub name: String,

    /// Available qualification.
    pub qualifier: Option<String>,

    /// Type arguments.
    pub type_arguments: Vec<TypeIndex>,

    /// Pack arguments.
    pub pack_arguments: Vec<PackIndex>,

    /// Unexpanded classification.
    pub state: ApplicationState,
}

/// A native type with no available source-representable solution.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Unresolved {
    /// Unsolved free variable.
    Free,

    /// Blocked on a constraint.
    Blocked,

    /// Unforced lazy thunk.
    Lazy,

    /// Native error recovery.
    Error,

    /// Internal refinement sentinel.
    InternalRefinement,
}

/// An owned native type node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    /// Primitive or universal type.
    Primitive(Primitive),

    /// Exact singleton.
    Singleton(Singleton),

    /// Named type.
    Named(Named),

    /// Generic type.
    Generic(Generic),

    /// Function structure.
    Function(Function),

    /// Table structure.
    Table(Table),

    /// Ordered union members.
    Union(Vec<TypeIndex>),

    /// Ordered intersection members.
    Intersection(Vec<TypeIndex>),

    /// A table and its metatable.
    Metatable {
        /// Table type.
        table: TypeIndex,
        /// Metatable type.
        metatable: TypeIndex,
    },

    /// Type negation.
    Negation(TypeIndex),

    /// Unexpanded operation.
    Application(Application),

    /// A bound reference.
    Reference(TypeIndex),

    /// Unresolved native state.
    Unresolved(Unresolved),

    /// Extraction limit.
    Omitted,
}

/// An owned native type-pack node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Pack {
    /// A sequence with a separate tail.
    Sequence {
        /// Fixed elements.
        head: Vec<TypeIndex>,
        /// Optional tail.
        tail: Option<PackIndex>,
    },

    /// Repeated element type.
    Variadic {
        /// Element type.
        element: TypeIndex,
        /// Whether native presentation hides this tail.
        hidden: bool,
    },

    /// Generic pack.
    Generic(Generic),

    /// Unreduced pack operation.
    Application(Application),

    /// Bound pack reference.
    Reference(PackIndex),

    /// Unresolved native state.
    Unresolved(Unresolved),

    /// Extraction limit.
    Omitted,
}

impl Snapshot {
    /// Whether every copied node is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.types.iter().any(|node| matches!(node, Type::Omitted))
            && !self.packs.iter().any(|node| matches!(node, Pack::Omitted))
    }

    /// Compare complete snapshots without treating matching omissions as evidence.
    #[must_use]
    pub fn equivalent(&self, other: &Self) -> bool {
        self.is_complete() && other.is_complete() && self == other
    }
}

fn invalid() -> Error {
    Error::Native {
        reason: "Invalid native presentation snapshot.".into(),
    }
}

fn optional(index: bindings::NativeIndex, length: usize) -> Result<Option<usize>, Error> {
    if index.has_value {
        reference(index.index, length).map(Some)
    } else if index.index == 0 {
        Ok(None)
    } else {
        Err(invalid())
    }
}

fn reference(index: usize, length: usize) -> Result<usize, Error> {
    if index < length {
        Ok(index)
    } else {
        Err(invalid())
    }
}

fn references(indices: Vec<usize>, length: usize) -> Result<Vec<usize>, Error> {
    indices
        .into_iter()
        .map(|index| reference(index, length))
        .collect()
}

fn one<Value>(mut values: Vec<Value>) -> Result<Value, Error> {
    if values.len() == 1 {
        values.pop().ok_or_else(invalid)
    } else {
        Err(invalid())
    }
}

fn unresolved(value: NativeUnresolvedState) -> Result<Unresolved, Error> {
    match value {
        NativeUnresolvedState::Free => Ok(Unresolved::Free),
        NativeUnresolvedState::Blocked => Ok(Unresolved::Blocked),
        NativeUnresolvedState::Lazy => Ok(Unresolved::Lazy),
        NativeUnresolvedState::Error => Ok(Unresolved::Error),
        NativeUnresolvedState::InternalRefinement => Ok(Unresolved::InternalRefinement),
        _ => Err(invalid()),
    }
}

fn application(
    node: bindings::NativeApplication,
    types: usize,
    packs: usize,
) -> Result<Application, Error> {
    if node.name.is_empty() || (node.qualifier.has_value == node.qualifier.value.is_empty()) {
        return Err(invalid());
    }

    Ok(Application {
        name: node.name,
        qualifier: node.qualifier.has_value.then_some(node.qualifier.value),
        type_arguments: references(node.type_arguments, types)?,
        pack_arguments: references(node.pack_arguments, packs)?,
        state: match node.state {
            NativeApplicationState::Pending => ApplicationState::Pending,
            NativeApplicationState::Unreduced => ApplicationState::Unreduced,
            _ => return Err(invalid()),
        },
    })
}

fn type_shape(node: &bindings::NativeType) -> Result<(), Error> {
    let can_have_types = matches!(
        node.kind,
        NativeTypeKind::Named
            | NativeTypeKind::Union
            | NativeTypeKind::Intersection
            | NativeTypeKind::Metatable
            | NativeTypeKind::Negation
            | NativeTypeKind::Reference
    );

    let can_have_packs = node.kind == NativeTypeKind::Named;
    let can_have_name = matches!(node.kind, NativeTypeKind::Named | NativeTypeKind::Generic);

    let can_have_bytes =
        node.kind == NativeTypeKind::Singleton && node.state == NativeSingletonState::String.repr;

    if (!can_have_types && !node.types.is_empty())
        || (!can_have_packs && !node.packs.is_empty())
        || (!can_have_name && !node.name.is_empty())
        || (!can_have_bytes && !node.bytes.is_empty())
        || (node.kind != NativeTypeKind::Named
            && (node.expansion.has_value || node.expansion.index != 0))
        || (node.kind != NativeTypeKind::Named
            && (node.qualifier.has_value || !node.qualifier.value.is_empty()))
        || (!node.qualifier.has_value && !node.qualifier.value.is_empty())
        || (node.kind == NativeTypeKind::Named && node.name.is_empty())
        || (node.kind == NativeTypeKind::Generic && node.identity >= 2_050)
        || (node.kind == NativeTypeKind::Generic
            && (node.has_explicit_name == node.name.is_empty()))
        || (node.kind == NativeTypeKind::Named
            && node.qualifier.has_value
            && node.qualifier.value.is_empty())
        || (node.kind != NativeTypeKind::Generic && (node.identity != 0 || node.has_explicit_name))
        || (!(node.kind == NativeTypeKind::Singleton
            && node.state == NativeSingletonState::Boolean.repr)
            && node.boolean)
        || (!matches!(
            node.kind,
            NativeTypeKind::Primitive
                | NativeTypeKind::Singleton
                | NativeTypeKind::Named
                | NativeTypeKind::Unresolved
        ) && node.state != 0)
    {
        return Err(invalid());
    }

    Ok(())
}

fn pack_shape(node: &bindings::NativePack) -> Result<(), Error> {
    if (!matches!(
        node.kind,
        NativePackKind::Sequence | NativePackKind::Variadic
    ) && !node.types.is_empty())
        || (node.kind != NativePackKind::Reference && !node.packs.is_empty())
        || (node.kind != NativePackKind::Generic
            && (!node.name.is_empty() || node.has_explicit_name))
        || (node.kind != NativePackKind::Sequence && (node.tail.has_value || node.tail.index != 0))
        || (node.kind != NativePackKind::Variadic && node.is_hidden)
        || (node.kind == NativePackKind::Generic
            && (node.identity >= 2_050 || (!node.has_explicit_name && !node.name.is_empty())))
        || (node.kind == NativePackKind::Generic
            && (node.has_explicit_name == node.name.is_empty()))
        || (node.kind != NativePackKind::Generic && node.identity != 0)
        || (node.kind != NativePackKind::Unresolved && node.state != 0)
        || (node.kind == NativePackKind::Unresolved
            && !matches!(
                NativeUnresolvedState { repr: node.state },
                NativeUnresolvedState::Free
                    | NativeUnresolvedState::Blocked
                    | NativeUnresolvedState::Error
            ))
    {
        return Err(invalid());
    }

    Ok(())
}

impl TryFrom<bindings::NativeSnapshot> for Snapshot {
    type Error = Error;

    fn try_from(snapshot: bindings::NativeSnapshot) -> Result<Self, Error> {
        let types = snapshot.types.len();
        let packs = snapshot.packs.len();

        let (maximum_nodes, maximum_edges, maximum_bytes) = if snapshot.is_expanded {
            (2_050, 16_384, 262_144)
        } else {
            (258, 1_024, 16_384)
        };

        if types + packs > maximum_nodes {
            return Err(invalid());
        }

        let root = if snapshot.is_root_pack {
            Subject::Pack(reference(snapshot.root, packs)?)
        } else {
            Subject::Type(reference(snapshot.root, types)?)
        };

        let mut edges = 0_usize;
        let mut bytes = 0_usize;
        let mut converted_types = Vec::with_capacity(types);

        for node in snapshot.types {
            type_shape(&node)?;

            if node.kind == NativeTypeKind::Generic && node.identity != converted_types.len() {
                return Err(invalid());
            }

            edges += node.types.len() + node.packs.len() + usize::from(node.expansion.has_value);
            bytes += node.name.len() + node.qualifier.value.len() + node.bytes.len();
            let function_count = usize::from(node.kind == NativeTypeKind::Function);
            let table_count = usize::from(node.kind == NativeTypeKind::Table);
            let application_count = usize::from(node.kind == NativeTypeKind::Application);

            if node.functions.len() != function_count
                || node.tables.len() != table_count
                || node.applications.len() != application_count
            {
                return Err(invalid());
            }

            let converted = match node.kind {
                NativeTypeKind::Primitive => {
                    let state = NativePrimitiveState { repr: node.state };

                    Type::Primitive(match state {
                        NativePrimitiveState::Nil => Primitive::Nil,
                        NativePrimitiveState::Boolean => Primitive::Boolean,
                        NativePrimitiveState::Number => Primitive::Number,
                        NativePrimitiveState::Integer => Primitive::Integer,
                        NativePrimitiveState::String => Primitive::String,
                        NativePrimitiveState::Thread => Primitive::Thread,
                        NativePrimitiveState::Function => Primitive::Function,
                        NativePrimitiveState::Table => Primitive::Table,
                        NativePrimitiveState::Buffer => Primitive::Buffer,
                        NativePrimitiveState::Any => Primitive::Any,
                        NativePrimitiveState::Unknown => Primitive::Unknown,
                        NativePrimitiveState::Never => Primitive::Never,
                        _ => return Err(invalid()),
                    })
                }

                NativeTypeKind::Singleton => {
                    let state = NativeSingletonState { repr: node.state };

                    Type::Singleton(match state {
                        NativeSingletonState::Boolean => Singleton::Boolean(node.boolean),
                        NativeSingletonState::String => Singleton::String(node.bytes),
                        _ => return Err(invalid()),
                    })
                }

                NativeTypeKind::Named => {
                    let provenance = NativeProvenance { repr: node.state };

                    Type::Named(Named {
                        name: node.name,
                        qualifier: node.qualifier.has_value.then_some(node.qualifier.value),
                        provenance: match provenance {
                            NativeProvenance::Alias => Provenance::Alias,
                            NativeProvenance::External => Provenance::External,
                            NativeProvenance::Synthetic => Provenance::Synthetic,
                            NativeProvenance::Unverified => Provenance::Unverified,
                            _ => return Err(invalid()),
                        },
                        type_arguments: references(node.types, types)?,
                        pack_arguments: references(node.packs, packs)?,
                        expansion: optional(node.expansion, types)?,
                    })
                }

                NativeTypeKind::Generic => Type::Generic(Generic {
                    identity: node.identity,
                    name: node.has_explicit_name.then_some(node.name),
                }),

                NativeTypeKind::Function => {
                    let function = one(node.functions)?;

                    if function
                        .argument_names
                        .iter()
                        .any(|name| !name.has_value && !name.value.is_empty())
                    {
                        return Err(invalid());
                    }

                    edges += function.generics.len() + function.generic_packs.len() + 2;

                    bytes += function
                        .argument_names
                        .iter()
                        .map(|name| size_of::<usize>() + name.value.len())
                        .sum::<usize>();

                    Type::Function(Function {
                        generics: references(function.generics, types)?,
                        generic_packs: references(function.generic_packs, packs)?,
                        arguments: reference(function.arguments, packs)?,
                        returns: reference(function.returns, packs)?,
                        argument_names: function
                            .argument_names
                            .into_iter()
                            .map(|name| name.has_value.then_some(name.value))
                            .collect(),
                        checked: function.is_checked,
                    })
                }

                NativeTypeKind::Table => {
                    let table = one(node.tables)?;

                    if table.indexers.len() > 1 {
                        return Err(invalid());
                    }

                    let properties = table
                        .properties
                        .into_iter()
                        .map(|property| {
                            edges += usize::from(property.read.has_value)
                                + usize::from(property.write.has_value);

                            bytes += property.name.len();
                            let read = optional(property.read, types)?;
                            let write = optional(property.write, types)?;

                            if (read.is_none() && write.is_none())
                                || (property.is_shared && (read.is_none() || read != write))
                            {
                                return Err(invalid());
                            }

                            Ok(TypeProperty {
                                name: property.name,
                                read,
                                write,
                                shared: property.is_shared,
                            })
                        })
                        .collect::<Result<_, Error>>()?;

                    let indexer = table
                        .indexers
                        .into_iter()
                        .map(|indexer| {
                            edges += 2;

                            Ok(Indexer {
                                key: reference(indexer.key, types)?,
                                value: reference(indexer.value, types)?,
                                readonly: indexer.is_read_only,
                            })
                        })
                        .next()
                        .transpose()?;

                    Type::Table(Table {
                        properties,
                        indexer,
                        state: match table.state {
                            NativeTableState::Sealed => TableState::Sealed,
                            NativeTableState::Unsealed => TableState::Unsealed,
                            NativeTableState::Free => TableState::Free,
                            NativeTableState::Generic => TableState::Generic,
                            NativeTableState::Exact => TableState::Exact,
                            _ => return Err(invalid()),
                        },
                    })
                }

                NativeTypeKind::Union => Type::Union(references(node.types, types)?),
                NativeTypeKind::Intersection => Type::Intersection(references(node.types, types)?),

                NativeTypeKind::Metatable => {
                    let members = references(node.types, types)?;

                    if members.len() != 2 {
                        return Err(invalid());
                    }

                    Type::Metatable {
                        table: members[0],
                        metatable: members[1],
                    }
                }

                NativeTypeKind::Negation => Type::Negation(reference(one(node.types)?, types)?),

                NativeTypeKind::Application => {
                    let value = one(node.applications)?;
                    edges += value.type_arguments.len() + value.pack_arguments.len();
                    bytes += value.name.len() + value.qualifier.value.len();

                    Type::Application(application(value, types, packs)?)
                }

                NativeTypeKind::Reference => Type::Reference(reference(one(node.types)?, types)?),

                NativeTypeKind::Unresolved => {
                    Type::Unresolved(unresolved(NativeUnresolvedState { repr: node.state })?)
                }

                NativeTypeKind::Omitted => Type::Omitted,
                _ => return Err(invalid()),
            };

            converted_types.push(converted);
        }

        let mut converted_packs = Vec::with_capacity(packs);

        for node in snapshot.packs {
            pack_shape(&node)?;

            if node.kind == NativePackKind::Generic && node.identity != converted_packs.len() {
                return Err(invalid());
            }

            edges += node.types.len() + node.packs.len() + usize::from(node.tail.has_value);
            bytes += node.name.len();

            if node.applications.len() != usize::from(node.kind == NativePackKind::Application) {
                return Err(invalid());
            }

            converted_packs.push(match node.kind {
                NativePackKind::Sequence => Pack::Sequence {
                    head: references(node.types, types)?,
                    tail: optional(node.tail, packs)?,
                },

                NativePackKind::Variadic => Pack::Variadic {
                    element: reference(one(node.types)?, types)?,
                    hidden: node.is_hidden,
                },

                NativePackKind::Generic => Pack::Generic(Generic {
                    identity: node.identity,
                    name: node.has_explicit_name.then_some(node.name),
                }),

                NativePackKind::Application => {
                    let value = one(node.applications)?;
                    edges += value.type_arguments.len() + value.pack_arguments.len();
                    bytes += value.name.len() + value.qualifier.value.len();

                    Pack::Application(application(value, types, packs)?)
                }

                NativePackKind::Reference => Pack::Reference(reference(one(node.packs)?, packs)?),

                NativePackKind::Unresolved => {
                    Pack::Unresolved(unresolved(NativeUnresolvedState { repr: node.state })?)
                }

                NativePackKind::Omitted => Pack::Omitted,
                _ => return Err(invalid()),
            });
        }

        bytes += edges * size_of::<usize>();

        if edges > maximum_edges || bytes > maximum_bytes {
            return Err(invalid());
        }

        let snapshot = Self {
            types: converted_types,
            packs: converted_packs,
            root,
        };

        for node in &snapshot.types {
            if let Type::Function(function) = node {
                for index in &function.generics {
                    let mut current = *index;

                    for _ in 0..=snapshot.types.len() {
                        match snapshot.types[current] {
                            Type::Generic(_) | Type::Omitted => break,
                            Type::Reference(next) => current = next,
                            _ => return Err(invalid()),
                        }
                    }

                    if !matches!(snapshot.types[current], Type::Generic(_) | Type::Omitted) {
                        return Err(invalid());
                    }
                }

                for index in &function.generic_packs {
                    let mut current = *index;

                    for _ in 0..=snapshot.packs.len() {
                        match snapshot.packs[current] {
                            Pack::Generic(_) | Pack::Omitted => break,
                            Pack::Reference(next) => current = next,
                            _ => return Err(invalid()),
                        }
                    }

                    if !matches!(snapshot.packs[current], Pack::Generic(_) | Pack::Omitted) {
                        return Err(invalid());
                    }
                }
            }
        }

        Ok(snapshot)
    }
}
