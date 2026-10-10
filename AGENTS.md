# Instar coding discipline

## Work from project precedent

Before editing, read the affected flow, its callers, and the nearest equivalent component. Identify how this project already expresses the same responsibility. Apply that convention across production code, tests, examples, and scripts.

Review authoring structure alongside behavior: directory ownership, module entry points, interface ownership, imports, names, visibility, type/implementation placement, helper order, API shape, and fixture organization. Passing a formatter or Clippy is not a substitute for this comparison.

Keep corrections complete across declarations, producers, callers, consumers, tests, and requested documentation. Preserve unrelated work. Use existing components and standard-library facilities before introducing an abstraction.

## Ownership and file anatomy

- Directories represent coherent subsystems; leaf files own one responsibility and its supporting types. Keep handwritten languages separate.
- Rust module entry points declare modules and expose the intended API. Implementation-only orchestration modules are private. A public inherent method does not require its implementation module to be public.
- Use the existing `mod.rs` convention for directory modules. Prefer one complete word for a new file or directory; obtain approval for multiword names. Preserve filenames required by tools and languages.
- Rust files use module declarations, grouped imports, constants/statics, related types and implementations, then free functions. Keep each type beside its implementations. Keep tightly local helpers local, such as a deserialization visitor belonging to one function.
- Within an inherent implementation, place constructors first, related public operations next, restricted operations next, and private helpers last. Group operations by responsibility. Preserve meaningful field, variant, and method order rather than alphabetizing them.
- Free functions expose the module's operations before their supporting private helpers. Test-local fixture helpers precede their tests.

## Imports and naming

- Group Rust imports as standard library, external crates, then local modules. Workspace crates belong in the external group. Follow `rustfmt.toml` for crate-level import granularity.
- Put ordinary imports at file scope. Import types directly or import a module for short, meaningful qualification such as `io::Result` or `mpsc::channel`. Use explicit ownership qualification or complete-word aliases when names collide.
- Internal code imports shared analysis contracts from `instar_shared`, their owner. Public facades may re-export them for consumers.
- Tests explicitly import their dependencies, including private production items accessed through local module paths. Do not inherit dependencies through wildcard imports.
- Use complete role names for project-controlled identifiers, including generic parameters and necessary explicit lifetimes. Examples are `Request`, `Response`, `Output`, `'source`, and `'tree`. Prefer lifetime elision when it expresses the same relationship.
- Trait methods, external overrides, wire formats, language constructs, and tools retain required spelling. A trait's example generic parameter spelling does not prescribe the local implementation's parameter names.
- Use one name for one concept. A prepared `Environment` includes declarations, capabilities, diagnostics, and completion state; a `Definition` is an actual declaration. Keep that distinction through orchestration and tests.

## API and implementation conventions

- Equivalent helpers use the same parameter order: primary inputs, mutable analysis/output state, then operation options and start time. Keep `options, started` adjacent and migrate callers with the declaration.
- Shared operation settings travel through `Options`. Concurrency comes from `Options.jobs`; helpers do not receive a duplicate concurrency argument. Preserve the remaining elapsed budget and cancellation signal across phases.
- Host-side isolated worker clients use infallible `Default` construction and start their subprocess on the first request. Reuse `instar_shared::process::Process`. Successful requests retain the process; interrupted or failed transport outcomes discard it. Report startup errors from the requesting operation.
- Native resource construction remains fallible when it performs an allocation or another fallible operation. Return types and error documentation describe actual work and failure timing.
- Host snapshot invalidation is infallible. Synchronization with the native worker retains its own real failure handling.
- Preserve the ownership distinction between `Rc` for thread-local state and `Arc` for cross-thread shared data. Preserve validation, interruption handling, diagnostic fidelity, deterministic output, and write protections.
- Do not add code comments unless requested or required by tooling. Keep required public documentation accurate, including error contracts.

## Native C++

- Give independently consumed components a self-contained interface header and implementation. Include the implemented interface explicitly. Let the configured formatter order and group includes.
- Keep private helpers local to their component. Align implementation order with the interface's meaningful declaration order, with constructors first and related operations together.
- Preserve external override names. Use the configured naming rules for project-controlled declarations and plain namespace closing delimiters.
- The private Rust `boundary` module owns CXX-generated code. Its existing `#[expect]` attributes document concrete generator constraints, including generated absolute paths. Keep these expectations on the module declaration in `crates/luau/src/lib.rs`; CXX rejects `#[expect]` on the bridge macro's input. They do not extend to ordinary bridge implementation code.

## Formatting and enforcement

Use installed tools and existing configuration. Ask before installations; do not edit dependencies, package caches, or generated/vendor files.

When formatting is part of a change, run `cargo fmt --all` for Rust first. For C++, feed each file to `clang-format --style=file` through stdin from that file's directory, without a filename argument or `--assume-filename`. Run Breathers last on the affected handwritten files; it owns the final spacing. Use the existing formatter configurations. For native Breathers processing, use the installed libclang and the mise-configured `LIBCLANG_PATH`.

Workspace Clippy denies `absolute_paths` and `single_char_lifetime_names` in addition to the existing lint groups and restrictions. Keep source-item ordering scoped to modules; semantic ordering remains an explicit review responsibility. The following also require review:

- Complete-word and role-appropriate names, including multi-character abbreviations and generic parameters.
- Canonical concept names, import ownership, parameter order, module visibility, and worker lifecycle policy.
- Explicit test imports: Clippy's wildcard-import lint exempts test crates.
- Accurate documentation and public fallibility contracts.
- Native header ownership and declaration/implementation alignment.

Do not add suppressions or weaken checks to accommodate handwritten code. The CXX generator expectations are narrowly scoped and reasoned. Investigate new tool conflicts and surface them rather than expanding those exceptions.

Verify significant behavioral changes with the relevant existing tests or end-to-end scenario. Do not add tests solely for coverage or run verification for simple prose edits. When workspace-wide lint enforcement changes, check all targets. Useful commands are `cargo clippy --workspace --all-targets --release --offline` and `cargo test --workspace --release --offline` when that scope is warranted. Report actual results and limits; remove temporary verification files.

For changed Nushell scripts, run `nufmt <files>`, `nu-lint <files>`, and `nu --ide-check 100 <file>` for each affected script. Respect the script's language-specific organization and the existing Breathers configuration.
