const ROOT = path self | path dirname | path dirname

# Materialize public declarations and documentation from a bundle.
export def materialize [destination: path]: record -> nothing {
    let bundle = $in

    for level in [none local plugin roblox] {
        let definition = $bundle.definitions | get --optional $level

        if $definition == null {
            error make $"Missing definitions for ($level)"
        }

        $definition | save --raw ($destination | path join $"($level).d.luau")
    }

    $bundle.documentation | to json --raw | save ($destination | path join documentation.json)
}

def main [
    destination: path # New directory for the public Pages assets.
]: nothing -> nothing {
    if ($destination | path exists) {
        error make 'Publication directory already exists'
    }

    mkdir $destination
    cp ($ROOT | path join schemas instar.schema.json) $destination
    open ($ROOT | path join generated bundle.json) | materialize $destination
}
