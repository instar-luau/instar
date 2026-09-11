const LEVELS = [none local plugin roblox]

def "main install" [
    directory: path # Directory for the released Linux executable.
]: nothing -> nothing {
    try {
        let tag = gh release view --json tagName --jq .tagName | str trim
        let archive = 'instar-x86_64-unknown-linux-gnu.tar.gz'
        gh release download $tag --pattern $archive --pattern checksums.txt --dir $directory
        let expected = open --raw ($directory | path join checksums.txt) | lines | parse '{checksum}  {name}' | where name == $archive | get checksum | first
        let actual = open --raw ($directory | path join $archive) | hash sha256

        if $actual != $expected { error make 'Instar release checksum mismatch' }

        tar -xzf ($directory | path join $archive) -C $directory
        ^($directory | path join instar) --version
    } catch {|failure| error make $failure }
}

def main [
    instar: path # Released Instar executable used to format generated declarations.
]: nothing -> nothing {
    try {
        nu scripts/roblox.nu
        main format $instar
    } catch {|failure| error make $failure }
}

def "main format" [
    instar: path # Instar executable used to format generated declarations.
]: nothing -> nothing {
    try {
        let files = $LEVELS | each {|level| 'generated' | path join $"($level).d.luau" }
        let executable = $instar | path expand --strict
        ^$executable format ...$files
        breathers ...$files

        let definitions = $LEVELS | reduce --fold {} {|level, result|
            $result | insert $level (open --raw ('generated' | path join $"($level).d.luau"))
        }

        let documentation = open generated/documentation.json
        {definitions: $definitions, documentation: $documentation} | to json --raw | save --force generated/bundle.json
    } catch {|failure| error make $failure }
}

def "main check" [
    instar: path # Instar executable used to check formatting and bundle consistency.
]: nothing -> nothing {
    use std/assert

    try {
        let root = $env.PWD
        let executable = $instar | path expand --strict
        let directory = mktemp --directory
        cp --recursive generated $directory
        cd $directory
        main format $executable
        let bundle = open generated/bundle.json

        for level in $LEVELS {
            assert (
                ($bundle.definitions | get --optional $level) == (open --raw ('generated' | path join $"($level).d.luau"))
            )
        }

        assert ($bundle.documentation == (open generated/documentation.json))
        let original = open --raw generated/bundle.json
        main format $executable
        assert ($original == (open --raw generated/bundle.json))
        cd $root
        rm --recursive $directory
    } catch {|failure| error make $failure }
}

def "main publish" []: nothing -> nothing {
    try {
        let files = $LEVELS | each {|level| 'generated' | path join $"($level).d.luau" } | append [generated/documentation.json generated/bundle.json]

        if (git status --porcelain -- ...$files | str trim | is-empty) { return }

        git add -- ...$files

        git -c 'user.name=github-actions[bot]' -c 'user.email=41898282+github-actions[bot]@users.noreply.github.com' commit --only -m 'chore: update generated assets' -- ...$files
        gh auth setup-git
        git push origin HEAD:main
        gh workflow run pages.yml
    } catch {|failure| error make $failure }
}
