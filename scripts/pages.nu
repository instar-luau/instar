const ROOT = path self | path dirname | path dirname

def main [
    destination: path # New directory for the public Pages assets.
]: nothing -> nothing {
    try {
        if ($destination | path exists) { error make 'Publication directory already exists' }

        mkdir $destination

        for source in [
            schemas/instar.schema.json
            generated/none.d.luau
            generated/local.d.luau
            generated/plugin.d.luau
            generated/roblox.d.luau
            generated/documentation.json
            generated/bundle.json
        ] {
            cp ($ROOT | path join $source) $destination
        }
    } catch {|failure| error make $failure }
}
