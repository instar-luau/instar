def main []: nothing -> string {
    let archive = "instar-cli-x86_64-unknown-linux-gnu.tar.xz"
    let directory = $env.RUNNER_TEMP | path join instar
    let output = "target/pages" | path expand

    gh release download $env.RELEASE_TAG --pattern $archive --pattern $"($archive).sha256" --dir $directory

    cd $directory
    sha256sum --check $"($archive).sha256"

    tar --extract --file $archive --strip-components=1

    let targets = if ($env.RELEASE_TAG | is-empty) {
        mkdir $output
        let schema = http get --raw https://instar-luau.github.io/instar/schema.json
        $schema | from json
        $schema | save --raw ($output | path join schema.json)
        [environments]
    } else {
        []
    }

    ./instar assets --generate ...$targets --output $output
}
