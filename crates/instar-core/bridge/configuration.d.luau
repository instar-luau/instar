export type LanguageMode = "strict" | "nonstrict" | "nocheck"

export type LintWarning = "*" | "UnknownGlobal" | "DeprecatedGlobal" | "GlobalUsedAsLocal" | "LocalShadow" | "SameLineStatement" | "MultiLineStatement" | "LocalUnused" | "FunctionUnused" | "ImportUnused" | "BuiltinGlobalWrite" | "PlaceholderRead" | "UnreachableCode" | "UnknownType" | "ForRange" | "UnbalancedAssignment" | "ImplicitReturn" | "DuplicateLocal" | "FormatString" | "TableLiteral" | "UninitializedLocal" | "DuplicateFunction" | "DeprecatedApi" | "TableOperations" | "DuplicateCondition" | "MisleadingAndOr" | "CommentDirective" | "IntegerParsing" | "ComparisonPrecedence" | "RedundantNativeAttribute"

export type LuauConfig = {
	languagemode: LanguageMode?,
	lint: { [LintWarning]: boolean }?,
	linterrors: boolean?,
	typeerrors: boolean?,
	globals: { string }?,
	aliases: { [string]: string }?,
}

export type Config = { luau: LuauConfig? }
