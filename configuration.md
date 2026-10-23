# Configuration

`instar.toml` holds project settings shared by the terminal commands and language server. [LSP settings](#lsp-settings) control editor behavior.

Fields are optional unless marked required. Unknown fields are errors.

## Contents

- [Loading and precedence](#loading-and-precedence)
- [File selection](#file-selection)
- [Type checking](#type-checking)
- [Formatting](#formatting)
- [Linting](#linting)
- [Module resolution](#module-resolution)
- [Environments](#environments)
- [Luau flags](#luau-flags)
- [Configuration schema](#configuration-schema)
- [LSP settings](#lsp-settings)

## Loading and precedence

Instar loads ancestor and nested `instar.toml` files. Precedence, lowest first:

1. Defaults.
2. `.luaurc` or `.config.luau`: checking mode, globals, and aliases.
3. `instar.toml`, outermost to innermost.
4. Command-line overrides.

Tables merge; arrays replace. Relative paths and patterns use the defining file's directory.

A directory cannot contain both `.luaurc` and `.config.luau`. Source checking directives override `check.mode`.

### Command-line overrides

`--configuration PATH` loads the named files instead of discovering `instar.toml`. Repeat it to load files in order. Instar still discovers `.luaurc` and `.config.luau`.

`--set FIELD=JSON` applies after files, in argument order. Relative paths use the project root, set with `--root`.

```sh
instar format --set format.width=100 --preview
```

`--include` and `--exclude` replace selection patterns for the command's operation. `check --mode` overrides `check.mode`. `lint --rule` enables only the named rules. These command-specific overrides apply after `--set` assignments.

## File selection

| Field     | Type             | Default | Meaning                                                                  |
| --------- | ---------------- | ------- | ------------------------------------------------------------------------ |
| `include` | Array of strings | Omitted | Shared inclusion patterns. Omission includes all supported source files. |
| `exclude` | Array of strings | Omitted | Shared exclusion patterns. Omission excludes nothing.                    |

`[check]`, `[index]`, `[format]`, and `[lint]` also accept these fields, each replacing its shared counterpart independently. `[index]` controls the workspace index used for symbols and automatic imports.

- Patterns use gitignore syntax. Command-line input globs are applied before these patterns.
- Negation removes paths from `include` and restores them in `exclude`. Excluded parents stay excluded. An include list with only negations starts with all paths included.
- `include = []` selects nothing; `exclude = []` clears inherited exclusions.
- Traversal includes hidden and gitignored `.luau` and `.lua` files, without following directory symlinks.
- Explicit files bypass selection for check, format, and lint. Index selection always applies.

## Type checking

`[check]`

| Field  | Type   | Default       | Meaning                                                    |
| ------ | ------ | ------------- | ---------------------------------------------------------- |
| `mode` | String | `"nonstrict"` | Checking mode for sources without an overriding directive. |

Modes are `"strict"`, `"nonstrict"`, and `"nocheck"`. `"nocheck"` disables ordinary type diagnostics.

## Formatting

`[format]`

| Field           | Type             | Default           | Accepted values or meaning                            |
| --------------- | ---------------- | ----------------- | ----------------------------------------------------- |
| `width`         | Positive integer | `120`             | Target line width.                                    |
| `indent_style`  | String           | `"tabs"`          | `"tabs"` or `"spaces"`.                               |
| `indent_width`  | Positive integer | `4`               | Indentation width.                                    |
| `line_ending`   | String           | `"lf"`            | `"lf"` or `"crlf"`.                                   |
| `quote_style`   | String           | `"prefer_double"` | Quoting policy, listed below.                         |
| `final_newline` | Boolean          | `true`            | Emit a final line ending for whole-source formatting. |

| Value             | Meaning                                                   |
| ----------------- | --------------------------------------------------------- |
| `"prefer_double"` | Prefer double quotes unless single quotes avoid escaping. |
| `"prefer_single"` | Prefer single quotes unless double quotes avoid escaping. |
| `"double"`        | Always use double quotes.                                 |
| `"single"`        | Always use single quotes.                                 |
| `"preserve"`      | Keep existing quotes.                                     |

### `[format.require]`

| Field         | Type                       | Default                          | Accepted values or meaning                          |
| ------------- | -------------------------- | -------------------------------- | --------------------------------------------------- |
| `order`       | String                     | `"preserve"`                     | `"preserve"`, `"alphabetical"`, or `"grouped"`.     |
| `groups`      | Array of strings or tables | `["alias", "relative", "other"]` | Require groups, checked in order. First match wins. |
| `blank_lines` | String                     | `"between_groups"`               | `"between_groups"` or `"none"`.                     |

`"grouped"` sorts alphabetically within each group.

Standard groups are `"alias"` for paths starting with `@`, `"relative"` for paths starting with `./` or `../`, and `"other"` for any remaining path. Unmatched requires form a trailing group.

A custom group requires `name` and `patterns`. Names must contain non-whitespace text and be unique across all groups. The names `alias`, `relative`, and `other` are reserved. `patterns` must contain at least one non-whitespace glob pattern. Globs match module paths.

```toml
[format.require]
order = "grouped"
groups = [{ name = "packages", patterns = ["@packages/**"] }, "alias", "relative", "other"]
```

## Linting

`[lint]`

| Field     | Type             | Default | Meaning                                       |
| --------- | ---------------- | ------- | --------------------------------------------- |
| `presets` | Array of strings | `[]`    | Presets applied in order.                     |
| `rules`   | Table            | `{}`    | Overrides by exact, case-sensitive rule name. |

By default, correctness rules are enabled at `"warn"` severity. Each preset replaces the previous rule selection:

| Preset          | Selection                          |
| --------------- | ---------------------------------- |
| `"correctness"` | Correctness rules.                 |
| `"recommended"` | Correctness and recommended rules. |
| `"all"`         | Every rule.                        |
| `"none"`        | No rules.                          |

`[lint.rules.NAME]` overrides presets:

| Field      | Type    | Default          | Meaning                                                                                |
| ---------- | ------- | ---------------- | -------------------------------------------------------------------------------------- |
| `enabled`  | Boolean | Preset selection | Enable or disable the rule.                                                            |
| `severity` | String  | `"warn"`         | `"error"`, `"warn"`, `"info"`, or `"hint"`. Does not enable a disabled rule by itself. |
| `settings` | Table   | Rule defaults    | Rule-specific configuration, described below.                                          |

A severity string is shorthand that also enables the rule:

```toml
[lint]
presets = ["recommended"]

[lint.rules]
unused_variable = "error"
LocalShadow = { enabled = false }
```

### Rule catalog

Each group lists the rules added by that preset.

Correctness rules, enabled by default:

```text
bad_string_escape
compare_nan
constant_condition
constant_table_comparison
length_as_condition
zero_step_loop
divide_by_zero
empty_if
empty_loop
if_same_then_else
mixed_table
almost_swapped
must_use
self_assignment
ignored_pcall_result
unscoped_variables
roblox_incorrect_color3_new_bounds
roblox_manual_fromscale_or_fromoffset
roblox_prefer_get_players
roblox_suspicious_udim2_new
unused_variable
restricted_globals
restricted_module_paths
UnknownGlobal
GlobalUsedAsLocal
LocalShadow
SameLineStatement
MultiLineStatement
LocalUnused
FunctionUnused
ImportUnused
BuiltinGlobalWrite
PlaceholderRead
UnreachableCode
UnknownType
ForRange
UnbalancedAssignment
ImplicitReturn
TableLiteral
UninitializedLocal
DuplicateFunction
DuplicateCondition
DuplicateLocal
MisleadingAndOr
IntegerParsing
ComparisonPrecedence
CommentDirective
RedundantNativeAttribute
```

Additional rules in `"recommended"`:

```text
non_const_require
type_check_inside_call
prefer_const
```

Additional rules in `"all"`:

```text
and_or_conditional
collapsible_if
else_after_return
negated_condition
parenthesized_conditions
if_expression_assignment
global_usage
loop_invariant_call
manual_table_clone
string_concat_in_loop
```

### Rule parameters

Only `unused_variable`, `prefer_const`, `restricted_globals`, and `restricted_module_paths` accept parameters. Other rules reject nonempty parameter tables.

For a rule named `NAME`, `[lint.rules.NAME.settings]` has these fields:

| Field        | Type   | Default  | Meaning                                                       |
| ------------ | ------ | -------- | ------------------------------------------------------------- |
| `rule`       | String | Required | Must equal `NAME`.                                            |
| `parameters` | Table  | `{}`     | Parameters for this rule. Unknown parameter names are errors. |

#### `unused_variable`

Under `[lint.rules.unused_variable.settings.parameters]`:

| Field            | Type    | Default | Meaning                                                                        |
| ---------------- | ------- | ------- | ------------------------------------------------------------------------------ |
| `parameters`     | Boolean | `false` | Check unused function parameters.                                              |
| `loop_variables` | Boolean | `false` | Check unused loop variables.                                                   |
| `ignore_pattern` | String  | `"^_"`  | Regular expression for ignored variable names. Invalid expressions are errors. |

#### `prefer_const`

Under `[lint.rules.prefer_const.settings.parameters]`:

| Field                       | Type    | Default | Meaning                                     |
| --------------------------- | ------- | ------- | ------------------------------------------- |
| `mutated_tables_stay_local` | Boolean | `false` | Keep bindings to mutated tables as `local`. |

#### `restricted_globals`

`[lint.rules.restricted_globals.settings.parameters.names]` maps global names to reasons shown in diagnostics. It defaults to an empty table.

Names must start with an ASCII letter or underscore and contain only ASCII letters, digits, or underscores. Reasons must contain non-whitespace text. Neither names nor reasons can contain NUL characters.

```toml
[lint.rules.restricted_globals.settings]
rule = "restricted_globals"

[lint.rules.restricted_globals.settings.parameters.names]
print = "Use the project logger."
```

#### `restricted_module_paths`

`[lint.rules.restricted_module_paths.settings.parameters.paths]` maps module paths to reasons shown in diagnostics. It defaults to an empty table.

Paths must be nonempty, have no surrounding whitespace, and contain no NUL characters. Reasons must contain non-whitespace text and no NUL characters.

```toml
[lint.rules.restricted_module_paths.settings]
rule = "restricted_module_paths"

[lint.rules.restricted_module_paths.settings.parameters.paths]
"../internal" = "Use the public module."
```

## Module resolution

`[resolve]`

| Field        | Type             | Default | Meaning                                                               |
| ------------ | ---------------- | ------- | --------------------------------------------------------------------- |
| `aliases`    | Table of strings | `{}`    | Alias names mapped to directories, merged with native Luau aliases.   |
| `sourcemaps` | Array of paths   | Omitted | Explicit Roblox sourcemaps. Omission allows automatic discovery.      |
| `generator`  | Table            | Omitted | Explicit sourcemap command. Omission allows automatic Rojo discovery. |

Alias names are case-insensitive; duplicates differing only in case are errors.

### Sourcemap discovery

- By default, Instar generates sourcemaps from `*.project.json` using Rojo on `PATH`. Shared `exclude` patterns filter project discovery.
- Without a generator, it searches for the nearest ancestor `sourcemap.json`. Paths inside that map use the map's directory.
- Setting `sourcemaps` disables automatic discovery. `[]` disables it entirely. Paths inside explicit maps use the workspace root.
- An explicit generator replaces Rojo discovery and can run alongside explicit maps. An empty command still allows ancestor map discovery when `sourcemaps` is omitted.

### `[resolve.generator]`

| Field     | Type             | Default | Meaning                                                      |
| --------- | ---------------- | ------- | ------------------------------------------------------------ |
| `command` | Array of strings | `[]`    | Executable followed by arguments. Empty disables generation. |
| `watch`   | Array of strings | `[]`    | Additional arguments for streaming watch mode.               |

Runs from the configuration file's directory, or the project root for command-line overrides. Writes JSON to stdout, one sourcemap per line in streaming watch mode. The executable must be nonblank; arguments cannot contain NUL.

```toml
[resolve.generator]
command = ["rojo", "sourcemap", "default.project.json", "--include-non-scripts"]
watch = ["--watch"]
```

Generators execute local programs. Use configuration from sources you trust.

## Environments

`[environment]`

| Field          | Type             | Default  | Meaning                                                                        |
| -------------- | ---------------- | -------- | ------------------------------------------------------------------------------ |
| `roblox`       | Boolean          | Omitted  | Automatic Roblox selection. `true` enables it explicitly. `false` disables it. |
| `security`     | String           | `"none"` | Roblox capability profile.                                                     |
| `globals`      | Array of strings | `[]`     | Additional declared global names, replacing inherited globals.                 |
| `capabilities` | Path             | Omitted  | JSON file supplying custom class capabilities.                                 |
| `groups`       | Array of tables  | `[]`     | Ordered custom declaration and documentation groups.                           |

Roblox is enabled automatically when sourcemaps or generators are present. `true` enables declarations without a sourcemap; `false` disables Roblox support.

Security profiles are `"none"` for standard scripts, `"local"` for local scripts, `"plugin"` for plugins, and `"roblox"` for Roblox-internal capabilities.

### `[[environment.groups]]`

| Field           | Type           | Default  | Meaning                                                                |
| --------------- | -------------- | -------- | ---------------------------------------------------------------------- |
| `namespace`     | String         | Required | Custom documentation namespace beginning with `@`, such as `@project`. |
| `definitions`   | Array of paths | `[]`     | Ordered Luau declaration files.                                        |
| `documentation` | Array of paths | `[]`     | Ordered JSON documentation files with prepared Markdown.               |

Namespace schema: `@[A-Za-z0-9_-]+`. Documentation JSON maps symbols to values; the last file wins for duplicate symbols.

```toml
[[environment.groups]]
namespace = "@project"
definitions = ["types/project.d.luau"]
documentation = ["types/documentation.json"]
```

### Custom capabilities

`environment.capabilities` replaces the class capability list. All fields shown below are required except `properties`, which defaults to `[]`.

```json
{
  "classes": [
    {
      "name": "Example",
      "service": false,
      "creatable": true,
      "properties": [{ "name": "Value", "read": true, "write": false }]
    }
  ]
}
```

## Luau flags

`[flags]` maps bundled Luau flag names to booleans or signed 32-bit integers. Default: `{}`, keeping upstream defaults. Use registered names without `FFlag` or `FInt` prefixes; names and types must match.

All modules and concurrent frontends must use the same effective flags, including across nested configurations and LSP workspaces.

## Configuration schema

Generate `assets/schema.json` for a TOML language server:

```sh
instar assets --generate schema --output assets
```

## LSP settings

Send LSP settings in your client's `instar` section:

```json
{
  "instar": {
    "diagnostic_scope": "workspace",
    "import": {
      "exclude": ["src/internal/**"]
    },
    "hints": {
      "parameter_names": false
    }
  }
}
```

### Delivery and precedence

- `initializationOptions` takes the settings directly, without an `instar` wrapper.
- `workspace/configuration` requests the `instar` section per workspace folder.
- `workspace/didChangeConfiguration` accepts wrapped or unwrapped settings. Folder responses override notifications; null responses fall back to notifications. If both are absent, existing settings remain.
- Settings replace the previous object. Omitted fields reset to defaults.

### Editor fields

| Field                 | Type               | Default        | Meaning                                                                       |
| --------------------- | ------------------ | -------------- | ----------------------------------------------------------------------------- |
| `diagnostic_scope`    | String             | `"open_files"` | `"open_files"` or `"workspace"`. Controls type-checking and lint diagnostics. |
| `completion_snippets` | Boolean            | `true`         | Allow completion snippets when the client supports them.                      |
| `import`              | Object             | Defaults below | Automatic import completion settings.                                         |
| `hints`               | Object of booleans | All enabled    | Inlay hint settings.                                                          |

`"workspace"` adds project-selected files to open-document checking and linting when a workspace directory is available.

### `import`

| Field     | Type             | Default    | Meaning                                                                              |
| --------- | ---------------- | ---------- | ------------------------------------------------------------------------------------ |
| `enabled` | Boolean          | `true`     | Offer automatic imports.                                                             |
| `require` | String           | `"string"` | `"string"` for module-path requires or `"instance"` for Roblox instance expressions. |
| `binding` | String           | `"local"`  | `"local"` or `"const"` for inserted bindings.                                        |
| `include` | Array of strings | `[]`       | Allow modules matching any pattern. Empty allows all.                                |
| `exclude` | Array of strings | `[]`       | Exclude modules matching any pattern.                                                |

Import globs match paths relative to the workspace root, without gitignore negation. Patterns must be nonempty. Exclusions override inclusions. Index exclusions also block module import suggestions. These filters apply only to modules, not Roblox services.

### `hints`

| Field             | Type    | Default | Meaning                                                        |
| ----------------- | ------- | ------- | -------------------------------------------------------------- |
| `variable_types`  | Boolean | `true`  | Show inferred variable type hints.                             |
| `parameter_names` | Boolean | `true`  | Show parameter name hints.                                     |
| `return_types`    | Boolean | `true`  | Show inferred return type hints.                               |
| `hide_obvious`    | Boolean | `true`  | Hide hints classified as obvious. Set to `false` to show them. |
