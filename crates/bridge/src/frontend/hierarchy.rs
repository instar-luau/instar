use std::io;

use instar_analysis::Location;
use serde::{Deserialize, Serialize};

use super::result::location;
use crate::boundary;

/// Native declaration role in a semantic hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum HierarchyKind {
    /// A function with a source body.
    Function,

    /// A nominal type with native inheritance information.
    Class,

    /// The top-level execution scope of a source module.
    Module,
}

/// A declaration anchored to a native source revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct HierarchyItem<Module = String> {
    /// Native declaration name.
    pub name: String,

    /// Native declaration role.
    pub kind: HierarchyKind,

    /// Complete declaration range.
    pub location: Location<Module>,

    /// Selection byte range within the declaration.
    pub selection: [usize; 2],
}

/// A call resolved to a native function body.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Call<Module = String> {
    /// Index of the calling scope.
    pub caller: usize,

    /// Index of the called function.
    pub callee: usize,

    /// Source range of the callee expression.
    pub location: Location<Module>,
}

/// A direct native nominal inheritance relationship.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Inheritance {
    /// Index of the derived nominal type.
    pub child: usize,

    /// Index of its direct parent type.
    pub parent: usize,
}

/// Detached native declarations, relationships, and an optional position selection.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Hierarchy<Module = String> {
    /// Declarations addressed by the relationship indices.
    pub items: Vec<HierarchyItem<Module>>,

    /// Statically resolved calls.
    pub calls: Vec<Call<Module>>,

    /// Direct nominal inheritance relationships.
    pub inheritance: Vec<Inheritance>,

    /// Indices selected by the requested position.
    pub selected: Vec<usize>,
}

impl<Module> Hierarchy<Module> {
    pub(super) fn validate(&self) -> io::Result<()> {
        if self.selected.iter().any(|&index| index >= self.items.len())
            || self.calls.iter().any(|call| {
                self.items.get(call.caller).is_none_or(|item| {
                    !matches!(item.kind, HierarchyKind::Function | HierarchyKind::Module)
                }) || self
                    .items
                    .get(call.callee)
                    .is_none_or(|item| item.kind != HierarchyKind::Function)
            })
            || self.inheritance.iter().any(|relation| {
                [relation.child, relation.parent].into_iter().any(|index| {
                    self.items
                        .get(index)
                        .is_none_or(|item| item.kind != HierarchyKind::Class)
                })
            })
            || self.items.iter().any(|item| {
                item.selection[0] < item.location.range[0]
                    || item.selection[0] > item.selection[1]
                    || item.selection[1] > item.location.range[1]
            })
        {
            return Err(io::Error::other("invalid native hierarchy relationship"));
        }

        Ok(())
    }
}

impl TryFrom<boundary::NativeHierarchy> for Hierarchy {
    type Error = io::Error;

    fn try_from(value: boundary::NativeHierarchy) -> io::Result<Self> {
        let items = value
            .items
            .into_iter()
            .map(|item| {
                let kind = match item.kind {
                    0 => HierarchyKind::Function,
                    1 => HierarchyKind::Class,
                    2 => HierarchyKind::Module,
                    _ => return Err(io::Error::other("unknown native hierarchy kind")),
                };

                Ok(HierarchyItem {
                    name: item.name,
                    kind,
                    location: location(item.location),
                    selection: [item.selection_start, item.selection_end],
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        let calls = value
            .calls
            .into_iter()
            .map(|call| Call {
                caller: call.caller,
                callee: call.callee,
                location: location(call.location),
            })
            .collect::<Vec<_>>();

        let inheritance = value
            .inheritance
            .into_iter()
            .map(|relation| Inheritance {
                child: relation.child,
                parent: relation.parent,
            })
            .collect::<Vec<_>>();

        let hierarchy = Self {
            items,
            calls,
            inheritance,
            selected: value.selected,
        };

        hierarchy.validate()?;

        Ok(hierarchy)
    }
}
