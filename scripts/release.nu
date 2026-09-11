const TARGETS = [x86_64-unknown-linux-gnu x86_64-pc-windows-msvc aarch64-apple-darwin]

def main []: nothing -> nothing {
    help main | print
}

def version [tag: string]: nothing -> nothing {
    try {
        let version = open Cargo.toml | get workspace.package.version

        if $tag != $"v($version)" { error make $"Release tag must be v($version)" }
    } catch {|failure| error make $failure }
}

def "main build" [
    target: string # Rust target triple to build.
    destination: path # Directory for the release archive.
]: nothing -> nothing {
    try {
        version $env.TAG

        if $target not-in $TARGETS { error make $"Unsupported release target: ($target)" }

        nu scripts/ci.nu $target
        cargo build --release --locked --package instar-cli --target $target

        let binary = if $target ends-with windows-msvc { 'instar.exe' } else { 'instar' }

        let directory = $"target/($target)/release"
        ^($directory | path join $binary) --version
        mkdir $destination
        tar -czf ($destination | path join $"instar-($target).tar.gz") -C $directory $binary
    } catch {|failure| error make $failure }
}

def "main publish" [
    tag: string # Existing version tag to release.
    directory: path # Directory containing all target archives.
]: nothing -> nothing {
    try {
        version $tag
        let archives = $TARGETS | each {|target| $directory | path join $"instar-($target).tar.gz" }

        let checksums = $archives | each {|archive|
            let checksum = open --raw $archive | hash sha256
            $"($checksum)  ($archive | path basename)"
        } | str join "\n"

        let manifest = $directory | path join checksums.txt
        $"($checksums)\n" | save --force $manifest
        gh release create $tag ...$archives $manifest --verify-tag --generate-notes
    } catch {|failure| error make $failure }
}
