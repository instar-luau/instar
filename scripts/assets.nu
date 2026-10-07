try {
    nu scripts/roblox.nu target/assets
} catch {|failure| exit $failure.exit_code }

let version = open Cargo.toml | get workspace.package.version

for namespace in [luau roblox] {
    mkdir $"target/pages/($namespace)"
    mv $"target/assets/($namespace)" $"target/pages/($namespace)/($version)"
}
