use std::{fmt, sync::LazyLock};

use instar_luau::{SemanticCategory, UpstreamWarningIdentity};
use instar_shared::Options;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    RuleInput, RuleResult, RuleSettings,
    rules::{assignments, bindings, calls, control, expressions, roblox},
};

const SYNTAX: Prerequisites = Prerequisites {
    syntax: true,
    bindings: false,
    environment: false,
    semantics: &[],
};

const LEXICAL: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: false,
    semantics: &[],
};

const ENVIRONMENT: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[],
};

const INFERRED: Prerequisites = Prerequisites {
    syntax: true,
    bindings: false,
    environment: true,
    semantics: &[SemanticCategory::Types],
};

const CALLS: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[SemanticCategory::Calls],
};

const TYPES: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[SemanticCategory::Bindings, SemanticCategory::Types],
};

const EFFECTS: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[
        SemanticCategory::Calls,
        SemanticCategory::Effects,
        SemanticCategory::Dependencies,
        SemanticCategory::Occurrences,
    ],
};

static CATALOG: LazyLock<Vec<RuleDefinition>> = LazyLock::new(|| {
    vec![
        RuleDefinition {
            identity: RuleIdentity::AlmostSwapped,
            category: RuleCategory::Assignments,
            description: "Assignments that overwrite a value before swapping it.",
            documentation: "Assignments that overwrite a value before swapping it. Example: `a=b; b=a`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(assignments::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::BadStringEscape,
            category: RuleCategory::Expressions,
            description: "Invalid or suspicious string escapes.",
            documentation: "Invalid or suspicious string escapes. Example: `\"\\\\q\"`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(expressions::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::CompareNan,
            category: RuleCategory::Expressions,
            description: "Comparisons against NaN have fixed outcomes.",
            documentation: "Comparisons against NaN have fixed outcomes. Example: `value == 0/0`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(expressions::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ConstantCondition,
            category: RuleCategory::Control,
            description: "Conditions with statically known truth values.",
            documentation: "Conditions with statically known truth values. Example: `if true then end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ConstantTableComparison,
            category: RuleCategory::Expressions,
            description: "Fresh table values compared by identity.",
            documentation: "Fresh table values compared by identity. Example: `{} == {}`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(expressions::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::LengthAsCondition,
            category: RuleCategory::Control,
            description: "Lengths used as booleans even though zero is truthy.",
            documentation: "Lengths used as booleans even though zero is truthy. Example: `if #items then end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::MismatchedArgumentCount,
            category: RuleCategory::Calls,
            description: "Calls with incompatible known argument constraints.",
            documentation: "Calls with incompatible lexical or imported argument constraints. Example: `local function f(a) end; f()`.",
            prerequisites: CALLS,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::MustUse,
            category: RuleCategory::Calls,
            description: "Discarded results from known result-producing operations.",
            documentation: "Discarded results from known result-producing operations. Example: `math.abs(-1)`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ZeroStepLoop,
            category: RuleCategory::Control,
            description: "Numeric loops with a statically zero step.",
            documentation: "Numeric loops with a statically zero step. Example: `for i=1,4,0 do end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::DivideByZero,
            category: RuleCategory::Expressions,
            description: "Statically identifiable division or modulo by zero.",
            documentation: "Statically identifiable division or modulo by zero. Example: `return 1/0`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(expressions::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::SelfAssignment,
            category: RuleCategory::Assignments,
            description: "Assignments of a binding to itself.",
            documentation: "Assignments of a binding to itself. Example: `a=a`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::Bindings,
            producer: RuleProducer::Instar(assignments::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::EmptyIf,
            category: RuleCategory::Control,
            description: "Conditional branches with empty bodies.",
            documentation: "Conditional branches with empty bodies. Example: `if flag then end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::EmptyLoop,
            category: RuleCategory::Control,
            description: "Empty loops that cannot make progress.",
            documentation: "Empty loops that cannot make progress. Example: `while flag do end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::IfSameThenElse,
            category: RuleCategory::Control,
            description: "Conditional branches with identical bodies.",
            documentation: "Conditional branches with identical bodies. Example: `if flag then f() else f() end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::IgnoredPcallResult,
            category: RuleCategory::Calls,
            description: "Protected-call success results that are ignored.",
            documentation: "Protected-call success results that are ignored. Example: `pcall(work)`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::MixedTable,
            category: RuleCategory::Expressions,
            description: "Table literals mixing positional and keyed entries.",
            documentation: "Table literals mixing positional and keyed entries. Example: `{1,key=2}`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(expressions::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::UnscopedVariables,
            category: RuleCategory::Bindings,
            description: "Assignments introducing undeclared global bindings.",
            documentation: "Assignments introducing undeclared global bindings. Example: `value=1`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::RobloxIncorrectColor3NewBounds,
            category: RuleCategory::Roblox,
            description: "Color3.new components outside zero through one.",
            documentation: "Color3.new components outside zero through one. Example: `Color3.new(255,0,0)`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(roblox::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::RobloxManualFromscaleOrFromoffset,
            category: RuleCategory::Roblox,
            description: "UDim2.new calls expressible as fromScale or fromOffset.",
            documentation: "UDim2.new calls expressible as fromScale or fromOffset. Example: `UDim2.new(1,0,1,0)`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(roblox::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::RobloxPreferGetPlayers,
            category: RuleCategory::Roblox,
            description: "Player enumeration better expressed with GetPlayers.",
            documentation: "Player enumeration better expressed with GetPlayers. Example: `game:GetService(\"Players\"):GetChildren()`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(roblox::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::RobloxSuspiciousUdim2New,
            category: RuleCategory::Roblox,
            description: "UDim2.new calls with suspicious scale and offset components.",
            documentation: "UDim2.new calls with suspicious scale and offset components. Example: `UDim2.new(1,2)`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(roblox::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ImplicitAnyLocal,
            category: RuleCategory::Bindings,
            description: "Unannotated local bindings with known inferred any types.",
            documentation: "Unannotated local bindings with known inferred any types. Example: `local value=unknown`.",
            prerequisites: TYPES,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ImplicitAnyParameter,
            category: RuleCategory::Bindings,
            description: "Unannotated parameters with known inferred any types.",
            documentation: "Unannotated parameters with known inferred any types. Example: `local function f(value) end`.",
            prerequisites: TYPES,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::AndOrConditional,
            category: RuleCategory::Expressions,
            description: "Conditional and/or expressions better expressed with if.",
            documentation: "Conditional and/or expressions better expressed with if. Example: `condition and left or right`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(expressions::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::CollapsibleIf,
            category: RuleCategory::Control,
            description: "Nested single-branch conditionals that can be combined.",
            documentation: "Nested single-branch conditionals that can be combined. Example: `if a then if b then work() end end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ElseAfterReturn,
            category: RuleCategory::Control,
            description: "Else branches following branches that always return.",
            documentation: "Else branches following branches that always return. Example: `if a then return else work() end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::IfExpressionAssignment,
            category: RuleCategory::Control,
            description: "Matching assignments expressible as conditional expressions.",
            documentation: "Matching assignments expressible as conditional expressions. Example: `if a then x=1 else x=2 end`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::NegatedCondition,
            category: RuleCategory::Control,
            description: "Negated conditions expressible by reversing branches.",
            documentation: "Negated conditions expressible by reversing branches. Example: `if not a then left() else right() end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::NonConstRequire,
            category: RuleCategory::Calls,
            description: "Require calls with nonconstant arguments.",
            documentation: "Require calls with nonconstant arguments. Example: `require(path)`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ParenthesizedConditions,
            category: RuleCategory::Control,
            description: "Unnecessary parentheses around conditions.",
            documentation: "Unnecessary parentheses around conditions. Example: `if (flag) then end`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::Syntax,
            producer: RuleProducer::Instar(control::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::GlobalUsage,
            category: RuleCategory::Bindings,
            description: "References to global bindings.",
            documentation: "References to global bindings. Example: `return global`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::TypeCheckInsideCall,
            category: RuleCategory::Calls,
            description: "Comparisons misplaced inside type or typeof calls.",
            documentation: "Comparisons misplaced inside type or typeof calls. Example: `type(value==\"number\")`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::LoopInvariantCall,
            category: RuleCategory::Calls,
            description: "Calls proven safe and independent of loop effects.",
            documentation: "Calls proven safe and independent of loop effects. Example: `while flag do math.abs(1) end`.",
            prerequisites: EFFECTS,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::ManualTableClone,
            category: RuleCategory::Expressions,
            description: "Table-copy loops expressible with table.clone.",
            documentation: "Table-copy loops expressible with table.clone. Example: `for key,value in pairs(source) do copy[key]=value end`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(expressions::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::StringConcatInLoop,
            category: RuleCategory::Calls,
            description: "Repeated string concatenation inside loops.",
            documentation: "Repeated string concatenation inside loops. Example: `while flag do text ..= item end`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::UnusedVariable,
            category: RuleCategory::Bindings,
            description: "Unread local bindings, parameters, and loop variables.",
            documentation: "Unread local bindings, parameters, and loop variables. Example: `local unused=1`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::UnusedBindings,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::PreferConst,
            category: RuleCategory::Bindings,
            description: "Initialized local bindings that are never reassigned.",
            documentation: "Initialized local bindings that are never reassigned. Example: `local fixed=1; return fixed`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::ImmutableBindings,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::Deprecated,
            category: RuleCategory::Bindings,
            description: "Project-deprecated symbol paths with replacements.",
            documentation: "Project-deprecated symbol paths with replacements. Example: `old.api()`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::DeprecatedSymbols,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::RestrictedGlobals,
            category: RuleCategory::Bindings,
            description: "Uses of project-restricted global names.",
            documentation: "Uses of project-restricted global names. Example: `print(value)`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::RestrictedGlobals,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(bindings::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::RestrictedModulePaths,
            category: RuleCategory::Calls,
            description: "Requires matching project-restricted module paths.",
            documentation: "Requires matching project-restricted module paths. Example: `require(\"./blocked\")`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::RestrictedImports,
            fixes: FixSupport::None,
            producer: RuleProducer::Instar(calls::evaluate),
        },
        RuleDefinition {
            identity: RuleIdentity::UnknownGlobal,
            category: RuleCategory::Bindings,
            description: "Unknown global references.",
            documentation: "Unknown global references. Example: `return missing`. Luau produces this warning as `UnknownGlobal`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "UnknownGlobal".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::DeprecatedGlobal,
            category: RuleCategory::Bindings,
            description: "Deprecated global references.",
            documentation: "Deprecated global references. Example: `collectgarbage()`. Luau produces this warning as `DeprecatedGlobal`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "DeprecatedGlobal".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::GlobalUsedAsLocal,
            category: RuleCategory::Bindings,
            description: "Global bindings better expressed as locals.",
            documentation: "Global bindings better expressed as locals. Example: `state=0; state+=1`. Luau produces this warning as `GlobalUsedAsLocal`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "GlobalUsedAsLocal".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::LocalShadow,
            category: RuleCategory::Bindings,
            description: "Local declarations shadowing existing bindings.",
            documentation: "Local declarations shadowing existing bindings. Example: `local value=1; do local value=2 end`. Luau produces this warning as `LocalShadow`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "LocalShadow".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::SameLineStatement,
            category: RuleCategory::Control,
            description: "Multiple statements on one line.",
            documentation: "Multiple statements on one line. Example: `print(1) print(2)`. Luau produces this warning as `SameLineStatement`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "SameLineStatement".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::MultiLineStatement,
            category: RuleCategory::Control,
            description: "Misleading multiline statement layout.",
            documentation: "Misleading multiline statement layout. Example: `local value = work\\n(1)`. Luau produces this warning as `MultiLineStatement`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "MultiLineStatement".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::LocalUnused,
            category: RuleCategory::Bindings,
            description: "Unused local bindings.",
            documentation: "Unused local bindings. Example: `local unused=1`. Luau produces this warning as `LocalUnused`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "LocalUnused".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::FunctionUnused,
            category: RuleCategory::Bindings,
            description: "Unused local functions.",
            documentation: "Unused local functions. Example: `local function unused() end`. Luau produces this warning as `FunctionUnused`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "FunctionUnused".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::ImportUnused,
            category: RuleCategory::Bindings,
            description: "Unused imports.",
            documentation: "Unused imports. Example: `local unused=require(\"./module\")`. Luau produces this warning as `ImportUnused`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "ImportUnused".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::BuiltinGlobalWrite,
            category: RuleCategory::Bindings,
            description: "Writes to builtin globals.",
            documentation: "Writes to builtin globals. Example: `print=function() end`. Luau produces this warning as `BuiltinGlobalWrite`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "BuiltinGlobalWrite".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::PlaceholderRead,
            category: RuleCategory::Bindings,
            description: "Reads from placeholder bindings.",
            documentation: "Reads from placeholder bindings. Example: `local _=1; return _`. Luau produces this warning as `PlaceholderRead`.",
            prerequisites: ENVIRONMENT,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "PlaceholderRead".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::UnreachableCode,
            category: RuleCategory::Control,
            description: "Unreachable code.",
            documentation: "Unreachable code. Example: `return 1; print(2)`. Luau produces this warning as `UnreachableCode`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "UnreachableCode".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::UnknownType,
            category: RuleCategory::Calls,
            description: "Unsupported names in runtime type tests.",
            documentation: "Unsupported names in runtime type tests. Example: `type(value)==\"integer\"`. Luau produces this warning as `UnknownType`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "UnknownType".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::ForRange,
            category: RuleCategory::Control,
            description: "Suspicious numeric loop ranges.",
            documentation: "Suspicious numeric loop ranges. Example: `for index=10,1 do end`. Luau produces this warning as `ForRange`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "ForRange".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::UnbalancedAssignment,
            category: RuleCategory::Assignments,
            description: "Assignment target and value mismatches.",
            documentation: "Assignment target and value mismatches. Example: `local left,right=1`. Luau produces this warning as `UnbalancedAssignment`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "UnbalancedAssignment".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::ImplicitReturn,
            category: RuleCategory::Control,
            description: "Inconsistent explicit and implicit returns.",
            documentation: "Inconsistent explicit and implicit returns. Example: `local function f(flag) if flag then return 1 end end`. Luau produces this warning as `ImplicitReturn`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "ImplicitReturn".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::DuplicateLocal,
            category: RuleCategory::Bindings,
            description: "Duplicate local declarations.",
            documentation: "Duplicate local declarations. Example: `local value,value=1,2`. Luau produces this warning as `DuplicateLocal`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "DuplicateLocal".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::FormatString,
            category: RuleCategory::Calls,
            description: "Invalid format strings or arguments.",
            documentation: "Invalid format strings or arguments. Example: `string.format(\"%d\", \"text\")`. Luau produces this warning as `FormatString`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "FormatString".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::TableLiteral,
            category: RuleCategory::Expressions,
            description: "Suspicious or duplicate table entries.",
            documentation: "Suspicious or duplicate table entries. Example: `{key=1,key=2}`. Luau produces this warning as `TableLiteral`.",
            prerequisites: INFERRED,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "TableLiteral".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::UninitializedLocal,
            category: RuleCategory::Bindings,
            description: "Reads before initialization.",
            documentation: "Reads before initialization. Example: `local value; return value`. Luau produces this warning as `UninitializedLocal`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "UninitializedLocal".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::DuplicateFunction,
            category: RuleCategory::Bindings,
            description: "Duplicate function definitions.",
            documentation: "Duplicate function definitions. Example: `function object:work() end; function object:work() end`. Luau produces this warning as `DuplicateFunction`.",
            prerequisites: LEXICAL,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "DuplicateFunction".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::DeprecatedApi,
            category: RuleCategory::Calls,
            description: "Deprecated API references.",
            documentation: "Deprecated API references. Example: `getfenv(1)`. Luau produces this warning as `DeprecatedApi`.",
            prerequisites: INFERRED,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "DeprecatedApi".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::TableOperations,
            category: RuleCategory::Calls,
            description: "Suspicious table operations.",
            documentation: "Suspicious table operations. Example: `table.insert(items, #items, value)`. Luau produces this warning as `TableOperations`.",
            prerequisites: INFERRED,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "TableOperations".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::DuplicateCondition,
            category: RuleCategory::Control,
            description: "Repeated conditional tests.",
            documentation: "Repeated conditional tests. Example: `if flag then left() elseif flag then right() end`. Luau produces this warning as `DuplicateCondition`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "DuplicateCondition".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::MisleadingAndOr,
            category: RuleCategory::Expressions,
            description: "Misleading conditional and/or results.",
            documentation: "Misleading conditional and/or results. Example: `flag and false or fallback`. Luau produces this warning as `MisleadingAndOr`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "MisleadingAndOr".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::CommentDirective,
            category: RuleCategory::Control,
            description: "Invalid analysis directives.",
            documentation: "Invalid analysis directives. Example: `--!unknown`. Luau produces this warning as `CommentDirective`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "CommentDirective".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::IntegerParsing,
            category: RuleCategory::Expressions,
            description: "Unexpected integer literal representation.",
            documentation: "Unexpected integer literal representation. Example: `9007199254740993`. Luau produces this warning as `IntegerParsing`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "IntegerParsing".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::ComparisonPrecedence,
            category: RuleCategory::Expressions,
            description: "Misleading comparison precedence.",
            documentation: "Misleading comparison precedence. Example: `not left == right`. Luau produces this warning as `ComparisonPrecedence`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "ComparisonPrecedence".to_owned(),
            }),
        },
        RuleDefinition {
            identity: RuleIdentity::RedundantNativeAttribute,
            category: RuleCategory::Control,
            description: "Redundant native compilation annotations.",
            documentation: "Redundant native compilation annotations. Example: `--!native\\n@native local function work() end`. Luau produces this warning as `RedundantNativeAttribute`.",
            prerequisites: SYNTAX,
            settings: RuleSchema::Basic,
            fixes: FixSupport::None,
            producer: RuleProducer::Upstream(UpstreamWarningIdentity {
                name: "RedundantNativeAttribute".to_owned(),
            }),
        },
    ]
});

/// Stable public identities of supported lint rules.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, JsonSchema,
)]
pub enum RuleIdentity {
    /// Assignments that overwrite a value before swapping it.
    #[serde(rename = "almost_swapped")]
    AlmostSwapped,

    /// Invalid or suspicious string escapes.
    #[serde(rename = "bad_string_escape")]
    BadStringEscape,

    /// Comparisons against NaN have fixed outcomes.
    #[serde(rename = "compare_nan")]
    CompareNan,

    /// Conditions with statically known truth values.
    #[serde(rename = "constant_condition")]
    ConstantCondition,

    /// Fresh table values compared by identity.
    #[serde(rename = "constant_table_comparison")]
    ConstantTableComparison,

    /// Lengths used as booleans even though zero is truthy.
    #[serde(rename = "length_as_condition")]
    LengthAsCondition,

    /// Calls with incompatible known lexical or imported argument constraints.
    #[serde(rename = "mismatched_argument_count")]
    MismatchedArgumentCount,

    /// Discarded results from known result-producing operations.
    #[serde(rename = "must_use")]
    MustUse,

    /// Numeric loops with a statically zero step.
    #[serde(rename = "zero_step_loop")]
    ZeroStepLoop,

    /// Statically identifiable division or modulo by zero.
    #[serde(rename = "divide_by_zero")]
    DivideByZero,

    /// Assignments of a binding to itself.
    #[serde(rename = "self_assignment")]
    SelfAssignment,

    /// Conditional branches with empty bodies.
    #[serde(rename = "empty_if")]
    EmptyIf,

    /// Empty loops that cannot make progress.
    #[serde(rename = "empty_loop")]
    EmptyLoop,

    /// Conditional branches with identical bodies.
    #[serde(rename = "if_same_then_else")]
    IfSameThenElse,

    /// Protected-call success results that are ignored.
    #[serde(rename = "ignored_pcall_result")]
    IgnoredPcallResult,

    /// Table literals mixing positional and keyed entries.
    #[serde(rename = "mixed_table")]
    MixedTable,

    /// Assignments introducing undeclared global bindings.
    #[serde(rename = "unscoped_variables")]
    UnscopedVariables,

    /// `Color3.new` components outside zero through one.
    #[serde(rename = "roblox_incorrect_color3_new_bounds")]
    RobloxIncorrectColor3NewBounds,

    /// `UDim2.new` calls expressible as `fromScale` or `fromOffset`.
    #[serde(rename = "roblox_manual_fromscale_or_fromoffset")]
    RobloxManualFromscaleOrFromoffset,

    /// Player enumeration better expressed with `GetPlayers`.
    #[serde(rename = "roblox_prefer_get_players")]
    RobloxPreferGetPlayers,

    /// `UDim2.new` calls with suspicious scale and offset components.
    #[serde(rename = "roblox_suspicious_udim2_new")]
    RobloxSuspiciousUdim2New,

    /// Unannotated local bindings with known inferred any types.
    #[serde(rename = "implicit_any_local")]
    ImplicitAnyLocal,

    /// Unannotated parameters with known inferred any types.
    #[serde(rename = "implicit_any_parameter")]
    ImplicitAnyParameter,

    /// Conditional and/or expressions better expressed with if.
    #[serde(rename = "and_or_conditional")]
    AndOrConditional,

    /// Nested single-branch conditionals that can be combined.
    #[serde(rename = "collapsible_if")]
    CollapsibleIf,

    /// Else branches following branches that always return.
    #[serde(rename = "else_after_return")]
    ElseAfterReturn,

    /// Matching assignments expressible as conditional expressions.
    #[serde(rename = "if_expression_assignment")]
    IfExpressionAssignment,

    /// Negated conditions expressible by reversing branches.
    #[serde(rename = "negated_condition")]
    NegatedCondition,

    /// Require calls with nonconstant arguments.
    #[serde(rename = "non_const_require")]
    NonConstRequire,

    /// Unnecessary parentheses around conditions.
    #[serde(rename = "parenthesized_conditions")]
    ParenthesizedConditions,

    /// References to global bindings.
    #[serde(rename = "global_usage")]
    GlobalUsage,

    /// Comparisons misplaced inside type or typeof calls.
    #[serde(rename = "type_check_inside_call")]
    TypeCheckInsideCall,

    /// Calls proven safe and independent of loop effects.
    #[serde(rename = "loop_invariant_call")]
    LoopInvariantCall,

    /// Table-copy loops expressible with table.clone.
    #[serde(rename = "manual_table_clone")]
    ManualTableClone,

    /// Repeated string concatenation inside loops.
    #[serde(rename = "string_concat_in_loop")]
    StringConcatInLoop,

    /// Unread local bindings, parameters, and loop variables.
    #[serde(rename = "unused_variable")]
    UnusedVariable,

    /// Initialized local bindings that are never reassigned.
    #[serde(rename = "prefer_const")]
    PreferConst,

    /// Project-deprecated symbol paths with replacements.
    #[serde(rename = "deprecated")]
    Deprecated,

    /// Uses of project-restricted global names.
    #[serde(rename = "restricted_globals")]
    RestrictedGlobals,

    /// Requires matching project-restricted module paths.
    #[serde(rename = "restricted_module_paths")]
    RestrictedModulePaths,

    /// Unknown global references.
    #[serde(rename = "UnknownGlobal")]
    UnknownGlobal,

    /// Deprecated global references.
    #[serde(rename = "DeprecatedGlobal")]
    DeprecatedGlobal,

    /// Global bindings better expressed as locals.
    #[serde(rename = "GlobalUsedAsLocal")]
    GlobalUsedAsLocal,

    /// Local declarations shadowing existing bindings.
    #[serde(rename = "LocalShadow")]
    LocalShadow,

    /// Multiple statements on one line.
    #[serde(rename = "SameLineStatement")]
    SameLineStatement,

    /// Misleading multiline statement layout.
    #[serde(rename = "MultiLineStatement")]
    MultiLineStatement,

    /// Unused local bindings.
    #[serde(rename = "LocalUnused")]
    LocalUnused,

    /// Unused local functions.
    #[serde(rename = "FunctionUnused")]
    FunctionUnused,

    /// Unused imports.
    #[serde(rename = "ImportUnused")]
    ImportUnused,

    /// Writes to builtin globals.
    #[serde(rename = "BuiltinGlobalWrite")]
    BuiltinGlobalWrite,

    /// Reads from placeholder bindings.
    #[serde(rename = "PlaceholderRead")]
    PlaceholderRead,

    /// Unreachable code.
    #[serde(rename = "UnreachableCode")]
    UnreachableCode,

    /// Unsupported names in runtime type tests.
    #[serde(rename = "UnknownType")]
    UnknownType,

    /// Suspicious numeric loop ranges.
    #[serde(rename = "ForRange")]
    ForRange,

    /// Assignment target and value mismatches.
    #[serde(rename = "UnbalancedAssignment")]
    UnbalancedAssignment,

    /// Inconsistent explicit and implicit returns.
    #[serde(rename = "ImplicitReturn")]
    ImplicitReturn,

    /// Duplicate local declarations.
    #[serde(rename = "DuplicateLocal")]
    DuplicateLocal,

    /// Invalid format strings or arguments.
    #[serde(rename = "FormatString")]
    FormatString,

    /// Suspicious or duplicate table entries.
    #[serde(rename = "TableLiteral")]
    TableLiteral,

    /// Reads before initialization.
    #[serde(rename = "UninitializedLocal")]
    UninitializedLocal,

    /// Duplicate function definitions.
    #[serde(rename = "DuplicateFunction")]
    DuplicateFunction,

    /// Deprecated API references.
    #[serde(rename = "DeprecatedApi")]
    DeprecatedApi,

    /// Suspicious table operations.
    #[serde(rename = "TableOperations")]
    TableOperations,

    /// Repeated conditional tests.
    #[serde(rename = "DuplicateCondition")]
    DuplicateCondition,

    /// Misleading conditional and/or results.
    #[serde(rename = "MisleadingAndOr")]
    MisleadingAndOr,

    /// Invalid analysis directives.
    #[serde(rename = "CommentDirective")]
    CommentDirective,

    /// Unexpected integer literal representation.
    #[serde(rename = "IntegerParsing")]
    IntegerParsing,

    /// Misleading comparison precedence.
    #[serde(rename = "ComparisonPrecedence")]
    ComparisonPrecedence,

    /// Redundant native compilation annotations.
    #[serde(rename = "RedundantNativeAttribute")]
    RedundantNativeAttribute,
}

impl RuleIdentity {
    /// Returns the canonical configuration and diagnostic name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AlmostSwapped => "almost_swapped",
            Self::BadStringEscape => "bad_string_escape",
            Self::CompareNan => "compare_nan",
            Self::ConstantCondition => "constant_condition",
            Self::ConstantTableComparison => "constant_table_comparison",
            Self::LengthAsCondition => "length_as_condition",
            Self::MismatchedArgumentCount => "mismatched_argument_count",
            Self::MustUse => "must_use",
            Self::ZeroStepLoop => "zero_step_loop",
            Self::DivideByZero => "divide_by_zero",
            Self::SelfAssignment => "self_assignment",
            Self::EmptyIf => "empty_if",
            Self::EmptyLoop => "empty_loop",
            Self::IfSameThenElse => "if_same_then_else",
            Self::IgnoredPcallResult => "ignored_pcall_result",
            Self::MixedTable => "mixed_table",
            Self::UnscopedVariables => "unscoped_variables",
            Self::RobloxIncorrectColor3NewBounds => "roblox_incorrect_color3_new_bounds",
            Self::RobloxManualFromscaleOrFromoffset => "roblox_manual_fromscale_or_fromoffset",
            Self::RobloxPreferGetPlayers => "roblox_prefer_get_players",
            Self::RobloxSuspiciousUdim2New => "roblox_suspicious_udim2_new",
            Self::ImplicitAnyLocal => "implicit_any_local",
            Self::ImplicitAnyParameter => "implicit_any_parameter",
            Self::AndOrConditional => "and_or_conditional",
            Self::CollapsibleIf => "collapsible_if",
            Self::ElseAfterReturn => "else_after_return",
            Self::IfExpressionAssignment => "if_expression_assignment",
            Self::NegatedCondition => "negated_condition",
            Self::NonConstRequire => "non_const_require",
            Self::ParenthesizedConditions => "parenthesized_conditions",
            Self::GlobalUsage => "global_usage",
            Self::TypeCheckInsideCall => "type_check_inside_call",
            Self::LoopInvariantCall => "loop_invariant_call",
            Self::ManualTableClone => "manual_table_clone",
            Self::StringConcatInLoop => "string_concat_in_loop",
            Self::UnusedVariable => "unused_variable",
            Self::PreferConst => "prefer_const",
            Self::Deprecated => "deprecated",
            Self::RestrictedGlobals => "restricted_globals",
            Self::RestrictedModulePaths => "restricted_module_paths",
            Self::UnknownGlobal => "UnknownGlobal",
            Self::DeprecatedGlobal => "DeprecatedGlobal",
            Self::GlobalUsedAsLocal => "GlobalUsedAsLocal",
            Self::LocalShadow => "LocalShadow",
            Self::SameLineStatement => "SameLineStatement",
            Self::MultiLineStatement => "MultiLineStatement",
            Self::LocalUnused => "LocalUnused",
            Self::FunctionUnused => "FunctionUnused",
            Self::ImportUnused => "ImportUnused",
            Self::BuiltinGlobalWrite => "BuiltinGlobalWrite",
            Self::PlaceholderRead => "PlaceholderRead",
            Self::UnreachableCode => "UnreachableCode",
            Self::UnknownType => "UnknownType",
            Self::ForRange => "ForRange",
            Self::UnbalancedAssignment => "UnbalancedAssignment",
            Self::ImplicitReturn => "ImplicitReturn",
            Self::DuplicateLocal => "DuplicateLocal",
            Self::FormatString => "FormatString",
            Self::TableLiteral => "TableLiteral",
            Self::UninitializedLocal => "UninitializedLocal",
            Self::DuplicateFunction => "DuplicateFunction",
            Self::DeprecatedApi => "DeprecatedApi",
            Self::TableOperations => "TableOperations",
            Self::DuplicateCondition => "DuplicateCondition",
            Self::MisleadingAndOr => "MisleadingAndOr",
            Self::CommentDirective => "CommentDirective",
            Self::IntegerParsing => "IntegerParsing",
            Self::ComparisonPrecedence => "ComparisonPrecedence",
            Self::RedundantNativeAttribute => "RedundantNativeAttribute",
        }
    }

    /// Resolves a canonical name without aliases.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        rules()
            .iter()
            .find(|rule| rule.identity.name() == name)
            .map(|rule| rule.identity)
    }
}

impl fmt::Display for RuleIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Evidence requested by a rule's producer.
#[derive(Clone, Copy, Debug)]
pub struct Prerequisites {
    /// Requires the captured lossless syntax.
    pub syntax: bool,

    /// Requires lexical declarations, references, and writes.
    pub bindings: bool,

    /// Requires the caller-established language environment.
    pub environment: bool,

    /// Required categories of detached semantic evidence.
    pub semantics: &'static [SemanticCategory],
}

/// Settings shape accepted by one rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleSchema {
    /// No rule-specific parameters.
    Basic,

    /// Unread binding exemptions.
    UnusedBindings,

    /// Immutable declaration policy.
    ImmutableBindings,

    /// Deprecated paths and replacements.
    DeprecatedSymbols,

    /// Restricted global names and reasons.
    RestrictedGlobals,

    /// Restricted import paths and reasons.
    RestrictedImports,
}

/// Supported correction guarantees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixSupport {
    /// No correction is currently supported.
    None,

    /// Supports syntax-preserving proposals.
    Syntax,

    /// Supports lexical-identity-preserving proposals.
    Bindings,
}

/// The sole producer of a catalog rule's findings.
#[derive(Clone)]
pub enum RuleProducer {
    /// Supplied-evidence Instar evaluator.
    Instar(fn(&RuleInput<'_, '_>, &RuleSettings, &Options) -> RuleResult),

    /// Native Luau warning identity; no duplicate Rust evaluator.
    Upstream(UpstreamWarningIdentity),
}

/// Responsibility category used independently of severity and producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleCategory {
    /// Binding declarations and references.
    Bindings,

    /// Assignment sequences and targets.
    Assignments,

    /// Value expressions and table construction.
    Expressions,

    /// Conditions, branches, and loops.
    Control,

    /// Calls, results, and module imports.
    Calls,

    /// Roblox-specific construction and enumeration.
    Roblox,
}

/// A supported rule and its public policy contract.
pub struct RuleDefinition {
    /// Canonical public identity.
    pub identity: RuleIdentity,

    /// Responsibility used for category selection.
    pub category: RuleCategory,

    /// Short catalog description.
    pub description: &'static str,

    /// Explanation and example for the finding.
    pub documentation: &'static str,

    /// Evidence required to establish full rule coverage.
    pub prerequisites: Prerequisites,

    /// Validated parameter shape.
    pub settings: RuleSchema,

    /// Supported proposal guarantees.
    pub fixes: FixSupport,

    /// Exclusive evaluator ownership.
    pub producer: RuleProducer,
}

/// Named groups applied before caller overrides.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Default correctness-focused rules.
    Correctness,

    /// Correctness rules plus selected maintainability suggestions.
    Recommended,

    /// Every supported catalog rule.
    All,

    /// No enabled rules, before explicit overrides.
    None,
}

/// Returns supported rules in deterministic catalog order.
#[must_use]
pub fn rules() -> &'static [RuleDefinition] {
    &CATALOG
}
