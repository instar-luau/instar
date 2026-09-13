def main [
    target: string # Rust target triple to check.
]: nothing -> nothing {
    nu scripts/roblox.nu check
    nu scripts/pages.nu check
    cargo test --workspace --release --locked --target $target
    cargo clippy --workspace --all-targets --release --locked --target $target -- -D warnings
}
