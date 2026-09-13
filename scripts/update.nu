const LEVELS = [none local plugin roblox]

use pages.nu materialize

def "main install" [
    directory: path # Directory for the released Linux executable.
]: nothing -> nothing {
    let tag = gh release view --json tagName --jq .tagName | str trim
    let archive = 'instar-x86_64-unknown-linux-gnu.tar.gz'
    gh release download $tag --pattern $archive --pattern checksums.txt --dir $directory
    let expected = open --raw ($directory | path join checksums.txt) | lines | parse '{checksum}  {name}' | where name == $archive | get checksum | first
    let actual = open --raw ($directory | path join $archive) | hash sha256

    if $actual != $expected { error make 'Instar release checksum mismatch' }

    tar -xzf ($directory | path join $archive) -C $directory
    ^($directory | path join instar) --version
}

def main [
    instar: path # Released Instar executable used to format generated declarations.
]: nothing -> nothing {
    nu scripts/roblox.nu
    main format $instar
}

def "main format" [
    instar: path # Instar executable used to format generated declarations.
]: nothing -> nothing {
    let bundle = open generated/bundle.json
    let directory = mktemp --directory --tmpdir-path generated
    $bundle | materialize $directory
    let files = $LEVELS | each {|level| $directory | path join $"($level).d.luau" }
    let executable = $instar | path expand --strict
    ^$executable format ...$files
    breathers ...$files

    let definitions = $LEVELS | reduce --fold {} {|level, result|
        $result | insert $level (open --raw ($directory | path join $"($level).d.luau"))
    }

    $bundle | update definitions $definitions | to json --raw | save --force generated/bundle.json
    rm --recursive $directory
}

def "main check" [
    instar: path # Instar executable used to check formatting and bundle consistency.
]: nothing -> nothing {
    use std/assert

    let root = $env.PWD
    let executable = $instar | path expand --strict
    let directory = mktemp --directory
    cp --recursive generated $directory
    cd $directory
    main format $executable
    let original = open --raw generated/bundle.json
    main format $executable
    assert equal $original (open --raw generated/bundle.json)
    cd $root
    rm --recursive $directory
}

def "main publish" []: nothing -> nothing {
    if (git status --porcelain -- generated/bundle.json | str trim | is-empty) { return }

    git add -- generated/bundle.json
    git -c 'user.name=github-actions[bot]' -c 'user.email=41898282+github-actions[bot]@users.noreply.github.com' commit --only -m 'chore: update generated assets' -- generated/bundle.json
    gh auth setup-git
    git push origin HEAD:main
    gh workflow run pages.yml
}
