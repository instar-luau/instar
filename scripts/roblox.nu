const DATA = path self | path dirname | path join roblox
const ROOT = '<<<ROOT>>>'

def identifier [name: string]: nothing -> bool {
    $name =~ '^[A-Za-z_][A-Za-z0-9_]*$' and $name not-in [
        and
        break
        do
        else
        elseif
        end
        'false'
        for
        function
        if
        in
        local
        nil
        not
        or
        repeat
        return
        then
        'true'
        until
        while
    ]
}

def property [name: string]: nothing -> string {
    if (identifier $name) { $name } else { $"[($name | to json --raw)]" }
}

def resolve [mappings: record]: record -> string {
    let value = $in

    if $value.Generic? != null {
        return $"{ ($value.Generic | str replace --all Enum. Enum) }"
    }

    if $value.Union? != null {
        return ($value.Union | each {|part| $part | resolve $mappings } | str join ' | ')
    }

    let variadic = $value.Tuple? | default $value.Variadic?

    if $variadic != null {
        return $"...($variadic | resolve $mappings)"
    }

    let name = $value.Declared? | default $value.Name?

    if $name == null {
        error make $"Missing type name: ($value | to json --raw)"
    }

    if $name ends-with '?' {
        return $"($value | upsert Name ($name | str substring ..-2) | reject --optional Declared | resolve $mappings)?"
    }

    if $name starts-with Enum. {
        return ($name | str replace --all Enum. Enum)
    }

    if $value.Category? == Enum {
        return $"Enum($name)"
    }

    $mappings | get --optional $name | default $name
}

def parameters [mappings: record]: list<any> -> string {
    let values = $in

    $values | enumerate | each {|entry|
        let parameter = $entry.item
        let type = $parameter.Type | resolve $mappings
        let variadic = ($type starts-with ...) or $parameter.Name == ...
        let last = $entry.index == (($values | length) - 1)

        if $variadic and $last {
            let element = $type | str replace --regex ^\.\.\. ''

            return $"...: ($element)"
        }

        let type = if $variadic { 'any' } else { $type }

        let type = if 'Default' in $parameter and not ($type ends-with '?') { $"($type)?" } else { $type }

        let name = if (identifier $parameter.Name) and $parameter.Name != self { $parameter.Name } else { $"argument($entry.index)" }

        $"($name): ($type)"
    } | str join ', '
}

def returns [member: record, mappings: record]: nothing -> string {
    let value = $member.TupleReturns? | default $member.ReturnType?

    if $value == null {
        return '()'
    }

    let values = [$value] | flatten

    let types = $values | enumerate | each {|entry|
        let type = $entry.item | resolve $mappings

        if ($type starts-with ...) and $entry.index < (($values | length) - 1) { 'any' } else { $type }
    }

    if ($types | length) == 1 { $types.0 } else { $"\(($types | str join ', ')\)" }
}

def repair [member: record, correction: record]: nothing -> record {
    mut result = $member

    for field in [ValueType ReturnType] {
        let patch = $correction | get --optional $field

        if $patch != null {
            $result = $result | upsert $field (($member | get --optional $field | default {}) | merge $patch)
        }
    }

    if $correction.TupleReturns? != null {
        $result = $result | upsert TupleReturns $correction.TupleReturns
    }

    if $correction.Parameters? != null {
        $result = $result | update Parameters {|value| $value.Parameters | each {|parameter|
            let patch = $correction.Parameters | where Name == $parameter.Name | get --optional 0

            if $patch == null {
                return $parameter
            }

            let updated = $parameter | merge ($patch | reject --optional Type)

            if $patch.Type? == null { $updated } else { $updated | update Type ($parameter.Type | merge $patch.Type) }
        } }
    }

    $result
}

def corrected [...patches: record]: list<any> -> list<any> {
    each {|class|
        let patch = $patches | where Name == $class.Name | get --optional 0

        $class | update Members {|class| $class.Members | each {|member|
            let correction = $patch.Members? | default [] | where Name == $member.Name | get --optional 0

            if $correction == null { $member } else { repair $member $correction }
        } }
    }
}

def access [member: record, ...labels: string]: nothing -> bool {
    if 'NotScriptable' in ($member.Tags? | default []) {
        return false
    }

    let security = $member.Security? | default None

    if ($security | describe) starts-with record {
        $security.Read in $labels or $security.Write in $labels
    } else {
        $security in $labels
    }
}

def member [value: record, mappings: record]: nothing -> string {
    let name = property $value.Name

    match $value.MemberType {
        Property => $"($name): ($value.ValueType | resolve $mappings)"
        Function => {
            let arguments = $value.Parameters | parameters $mappings

            let arguments = if $arguments == '' { 'self' } else { $"self, ($arguments)" }

            $"function ($name)\(($arguments)\): (returns $value $mappings)"
        }
        Callback => {
            let arguments = $value.Parameters | parameters $mappings | str replace --all '...: ' ...
            $"($name): \(($arguments)\) -> (returns $value $mappings)"
        }
        Event => {
            let payload = returns {
                TupleReturns: ($value.Parameters | get Type)
            } $mappings

            $"($name): RBXScriptSignal<($payload)>"
        }
        _ => { error make $"Unsupported member kind: ($value.MemberType)" }
    }
}

def inherited [name: string, ...classes: record]: record -> record {
    let class = $in
    let member = $class.Members | where Name == $name | get --optional 0

    if $member != null {
        return $member
    }

    let parent = $classes | where Name == ($class.Superclass? | default $ROOT) | get --optional 0

    if $parent == null {
        return {}
    }

    $parent | inherited $name ...$classes
}

def declaration [class: record, context: record]: nothing -> string {
    if $class.Name in $context.corrections.Exclusions.Types {
        return ''
    }

    let replacements = $context.members | get --optional $class.Name | default [] | reduce --fold {} {|line, result|
        let name = $line | parse --regex '^(?:function )?(?<name>[A-Za-z_][A-Za-z0-9_]*)' | get name | first
        let existing = $result | get --optional $name | default []
        $result | upsert $name ($existing | append $line)
    }

    mut lines = []

    for value in $class.Members {
        let allowed = access $value ...$context.labels

        if not $allowed {
            continue
        }

        if $value.Name in $replacements {
            if $value.Name not-in ($class.Members | take until {|entry| $entry == $value } | get Name) {
                $lines ++= ($replacements | get --optional $value.Name | default [])
            }
        } else {
            $lines ++= [
                (member $value $context.corrections.Types)
            ]
        }
    }

    for replacement in ($replacements | transpose name lines) {
        if $replacement.name in ($class.Members | each {|value| $value.Name }) {
            continue
        }

        let original = $class | inherited $replacement.name ...$context.classes

        if ($original | is-not-empty) {
            let allowed = access $original ...$context.labels

            if not $allowed {
                continue
            }
        }

        $lines ++= $replacement.lines
    }

    let parent = $class.Superclass? | default $ROOT

    let parent = if $parent == $ROOT { '' } else { $" extends ($parent)" }

    $"declare extern type ($class.Name)($parent) with\n\t($lines | str join "\n\t")\nend"
}

def constructor [class: record, corrections: record]: nothing -> string {
    if $class.Name in $corrections.Exclusions.Types {
        return ''
    }

    mut fields = $class.Members | where MemberType == Property | each {|value| member $value $corrections.Types }

    for group in (
        $class.Members
        | where MemberType == Function
        | group-by Name
        | transpose name overloads
    ) {
        let overloads = $group.overloads | each {|value|
            let arguments = $value.Parameters | parameters $corrections.Types | str replace --all '...: ' ...
            $"\(\(($arguments)\) -> (returns $value $corrections.Types)\)"
        } | str join ' & '

        $fields ++= [$"(property $group.name): ($overloads)"]
    }

    $"declare ($class.Name): {\n\t($fields | str join ",\n\t")\n}"
}

def blocks [source: string]: nothing -> list<any> {
    let separator = char --integer 0x1e
    let chunks = $source | str replace --all --regex '(?m)^((?:export )?type |declare |@)' ($separator + '$1') | split row $separator
    mut result = []
    mut attributes = ''

    for chunk in $chunks {
        let text = $chunk | str trim

        if $text == '' {
            continue
        }

        let header = $text | parse --regex '(?m)(?:^| )(?<kind>declare extern type|export type|type|declare function|declare) (?<name>[A-Za-z_][A-Za-z0-9_]*)' | get --optional 0

        if $header == null {
            if $text starts-with @ {
                $attributes += $"($text)\n"
            }

            continue
        }

        let kind = if $header.kind ends-with type { 'type' } else { 'value' }

        let parent = $text | parse --regex '^declare extern type \w+ extends (?<name>\w+)' | get name
        let defaults = $text | lines | first | parse --regex '<[^>]*=\s*(?<name>\w+)' | get name | where $it not-in [any unknown never string number boolean buffer thread vector]
        let dependencies = $defaults | append $parent

        $result ++= [
            {
                key: $"($kind):($header.name)"
                name: $header.name
                dependencies: $dependencies
                text: $"($attributes)($text)"
            }
        ]

        $attributes = ''
    }

    if $attributes != '' {
        error make 'Attribute without a declaration'
    }

    $result
}

def ordered [source: string]: nothing -> string {
    mut pending = blocks $source
    mut emitted = []
    mut result = []

    while ($pending | is-not-empty) {
        mut remaining = []

        for block in $pending {
            let known = $emitted

            if ($block.dependencies | any {|name| $name not-in $known }) {
                $remaining ++= [$block]
            } else {
                $result ++= [$block.text]

                if $block.key starts-with type: {
                    $emitted ++= [$block.name]
                }
            }
        }

        if ($remaining | length) == ($pending | length) {
            error make $"Unresolved class inheritance: ($remaining | get name | str join ', ')"
        }

        $pending = $remaining
    }

    $result | str join "\n\n"
}

def injected [source: string, context: record]: nothing -> string {
    let sections = $source | parse --regex '(?s)-- SECTION BEGIN: (?<name>[^\r\n]+)\r?\n(?<body>.*?)-- SECTION END: [^\r\n]+'
    let text = $sections | where name not-in $context.corrections.Exclusions.Sections | get body | str join "\n"
    let replaced = blocks $context.declarations | get key

    blocks $text | where key not-in $replaced and not ($it.key starts-with value: and $it.name in $context.corrections.Exclusions.Globals) | each {|block|
        if $block.name in $context.corrections.Exports and ($block.text starts-with 'type ') { $"export ($block.text)" } else { $block.text }
    } | str join "\n\n" | str replace --all Enum. Enum
}

def references [value: record, mappings: record]: nothing -> list<string> {
    if $value.Union? != null {
        return ($value.Union | each {|part| references $part $mappings } | flatten)
    }

    let nested = $value.Tuple? | default $value.Variadic?

    if $nested != null {
        return (references $nested $mappings)
    }

    let type = if $value.Generic? != null {
        {Name: $value.Generic} | resolve $mappings
    } else {
        $value | resolve $mappings
    }

    let type = $type | str replace --regex '[?]$' ''

    if (identifier $type) and $type not-in [
        string
        number
        boolean
        nil
        any
        unknown
        never
        buffer
        thread
        vector
    ] { [$type] } else { [] }
}

def canonical-symbol [value: string, roots: record]: nothing -> string {
    let symbol = $value | parse --regex '^@roblox/global/(?<root>[A-Za-z_][A-Za-z0-9_]*)(?<suffix>(?:[./].*)?)$' | get --optional 0

    if $symbol != null and $symbol.root in $roots.luau and $symbol.root not-in $roots.roblox {
        $"@luau/global/($symbol.root)($symbol.suffix)"
    } else {
        $value
    }
}

def canonical [value: oneof<record, list<any>, string, int, float, bool, nothing>, roots: record]: nothing -> oneof<record, list<any>, string, number, bool, nothing> {
    if ($value | describe) starts-with record {
        let keys = $value | columns

        $value | transpose key value | reduce --fold {} {|entry, result|
            let key = canonical-symbol $entry.key $roots

            if $key != $entry.key and $key in $keys {
                $result
            } else {
                $result | upsert $key (canonical $entry.value $roots)
            }
        }
    } else if ($value | describe) starts-with list or ($value | describe) starts-with table {
        $value | each {|entry| canonical $entry $roots }
    } else if ($value | describe) == string {
        canonical-symbol $value $roots
    } else {
        $value
    }
}

def main [
    destination: path # Directory for generated publishing artifacts.
]: nothing -> nothing {
    let corrections = open ($DATA | path join corrections.json)
    let types = open ($DATA | path join types.json)
    let tracker = 'https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox'
    let dump = http get --raw $"($tracker)/Full-API-Dump.json" | from json
    let source = http get --raw $"($tracker)/LuauTypes.d.luau"
    let documentation = http get --raw $"($tracker)/api-docs/en-us.json" | from json

    if not (($documentation | describe) starts-with record) {
        error make 'Invalid documentation object'
    }

    let classes = $dump.Classes | corrected ...$corrections.Classes
    let datatypes = $types.DataTypes | corrected ...$corrections.Classes
    let constructors = $types.Constructors | corrected ...$corrections.Classes
    let declarations = open --raw ($DATA | path join declarations.d.luau)

    let members = blocks (open --raw ($DATA | path join members.d.luau)) | reduce --fold {} {|block, result|
        if not ($block.text starts-with 'declare extern type ') or not ($block.text ends-with end) {
            error make $"Invalid member declaration: ($block.name)"
        }

        let lines = $block.text | lines | skip 1 | drop 1 | each {|line| $line | str trim } | where $it != ''
        mut signatures = []
        mut current = []
        mut depth = 0

        for line in $lines {
            if $depth == 0 and $line =~ '^(?:function\s+\w+|\w+\s*:)' and ($current | is-not-empty) {
                $signatures ++= [
                    ($current | str join "\n")
                ]

                $current = []
            }

            $current ++= [$line]
            let tokens = $line | parse --regex `(?<token>"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|--.*|[(){}\[\]])` | get token

            for token in $tokens {
                if $token in ['(' '{' '['] {
                    $depth += 1
                }

                if $token in [')' '}' ']'] {
                    $depth -= 1
                }

                if $depth < 0 {
                    error make $"Unbalanced member signature: ($line)"
                }
            }
        }

        if $depth != 0 {
            error make 'Unterminated member signature'
        }

        if ($current | is-not-empty) {
            $signatures ++= [
                ($current | str join "\n")
            ]
        }

        $result | upsert $block.name $signatures
    }

    let injections = injected $source {corrections: $corrections, declarations: $declarations}
    let declared = $"($declarations)\n($injections)"

    let vendor = $DATA | path dirname | path dirname | path join crates instar-bridge vendor luau Analysis src
    let embedded = open --raw ($vendor | path join EmbeddedBuiltinDefinitions.cpp)
    let builtin = open --raw ($vendor | path join BuiltinDefinitions.cpp)
    let registered = $builtin | parse --regex 'addGlobalBinding\(\s*globals,\s*"(?<name>[^"]+)"(?s:.*?)"@luau"' | get name
    let luau_roots = blocks $embedded | get name | append $registered | uniq

    let defined = [
        ...($classes | get Name)
        ...($datatypes | get Name)
        ...($dump.Enums | each {|value| $"Enum($value.Name)" })
        ...(
            $declared
            | parse --regex '(?:type|declare extern type) (?<name>[A-Za-z_][A-Za-z0-9_]*)'
            | get name
        )
    ]

    let referenced = [$classes $datatypes $constructors] | flatten | get Members | flatten | each {|value|
        [$value.ValueType? $value.ReturnType? $value.TupleReturns?] | append ($value.Parameters? | default [] | each {|parameter| $parameter.Type })
    } | flatten | flatten | compact | each {|value| references $value $corrections.Types } | flatten | uniq | sort

    let stubs = $referenced | where $it not-in $defined | each {|name| $"declare extern type ($name) with end" }

    let enums = $dump.Enums | each {|value| [
        $"declare extern type Enum($value.Name) extends EnumItem with end"
        $"declare extern type Enumeration($value.Name) extends Enum with"
        ...(
            $value.Items
            | each {|item| $"\t(property $item.Name): Enum($value.Name)" }
        )
        $"\tfunction GetEnumItems\(self\): {Enum($value.Name)}"
        $"\tfunction FromName\(self, name: string\): Enum($value.Name)?"
        $"\tfunction FromValue\(self, value: number\): Enum($value.Name)?"
        end
    ] | str join "\n" } | str join "\n"

    let fields = $dump.Enums | each {|value| $"\t(property $value.Name): Enumeration($value.Name)" } | str join "\n"
    let namespace = $"declare extern type Enumerations with\n\t[string]: Enum\n($fields)\n\tfunction GetEnums\(self\): {Enum}\nend\ndeclare Enum: Enumerations"

    let hierarchy = [
        None
        LocalUserSecurity
        PluginSecurity
        WritePlayerSecurity
        RobloxScriptSecurity
    ]

    let metadata = {
        services: (
            $classes
            | where ('Service' in ($it.Tags? | default []))
            | get Name
        )

        creatable_instances: (
            $classes
            | where ('NotCreatable' not-in ($it.Tags? | default []) and 'Service' not-in ($it.Tags? | default []))
            | get Name
        )
    } | to json --raw

    mut definitions = {}

    for level in [
        {name: None, file: none}
        {name: LocalUserSecurity, file: local}
        {name: PluginSecurity, file: plugin}
        {name: RobloxScriptSecurity, file: roblox}
    ] {
        let labels = $hierarchy | take until {|name| $name == $level.name } | append $level.name

        let context = {
            classes: $classes
            corrections: $corrections
            labels: $labels
            members: $members
        }

        let body = [
            ...$stubs
            $declarations
            $enums
            $namespace
            ...($datatypes | each {|value| declaration $value $context })
            $injections
            ...($classes | each {|value| declaration $value $context })
            ...($constructors | each {|value| constructor $value $corrections })
        ] | str join "\n" | ordered $in

        $definitions = $definitions | upsert $level.file $"--#METADATA#($metadata)\n($body)"
    }

    mkdir $destination

    for entry in ($definitions | transpose file source) {
        $entry.source | save --force ($destination | path join $"($entry.file).d.luau")
    }

    let roblox_roots = $definitions | values | each {|source| blocks $source | where key starts-with value: | get name } | flatten | uniq
    let canonical_documentation = canonical $documentation {luau: $luau_roots, roblox: $roblox_roots}
    $canonical_documentation | to json --raw | save --force ($destination | path join documentation.json)
}
