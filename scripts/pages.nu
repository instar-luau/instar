const ROOT = path self | path dirname | path dirname

def main [
    destination: path # New directory for the public Pages assets.
    instar: path # Released Instar executable used to format declarations.
]: nothing -> nothing {
    if ($destination | path exists) {
        error make 'Publication directory already exists'
    }

    try {
        nu ($ROOT | path join scripts roblox.nu) $destination
        let files = [none local plugin roblox] | each {|level| $destination | path join $"($level).d.luau" }
        let executable = $instar | path expand --strict
        ^$executable format ...$files
        breathers ...$files
        cp ($ROOT | path join schemas instar.schema.json) $destination
    } catch {|err|
        if ($destination | path exists) {
            rm --recursive $destination
        }

        error make $err
    }
}

def "main install" [
    directory: path # Directory for the released Linux executable.
]: nothing -> nothing {
    let tag = gh release view --json tagName --jq .tagName | str trim
    let archive = 'instar-x86_64-unknown-linux-gnu.tar.gz'
    gh release download $tag --pattern $archive --pattern checksums.txt --dir $directory
    let expected = open --raw ($directory | path join checksums.txt) | lines | parse '{checksum}  {name}' | where name == $archive | get checksum | first
    let actual = open --raw ($directory | path join $archive) | hash sha256

    if $actual != $expected {
        error make 'Instar release checksum mismatch'
    }

    tar -xzf ($directory | path join $archive) -C $directory
    ^($directory | path join instar) --version
}
