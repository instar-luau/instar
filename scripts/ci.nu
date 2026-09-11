def main [
    target: string # Rust target triple to check.
]: nothing -> nothing {
    try {
        nu scripts/roblox.nu check
        cargo test --workspace --locked --target $target
        cargo clippy --workspace --all-targets --locked --target $target -- -D warnings
    } catch {|failure| error make $failure }
}
