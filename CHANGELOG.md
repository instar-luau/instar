# Changelog
## [0.1.9](https://github.com/instar-luau/instar/compare/v0.1.8...v0.1.9) - 2026-10-26

### Fixed

- **core:** expand home-relative module aliases

## [0.1.8](https://github.com/instar-luau/instar/compare/v0.1.7...v0.1.8) - 2026-10-26

### Fixed

- **core:** accept Secret-valued HTTP request URLs
- **bridge:** narrow QueryDescendants class selectors

## [0.1.7](https://github.com/instar-luau/instar/compare/v0.1.6...v0.1.7) - 2026-10-26

### Fixed

- **core:** restart failed sourcemap watch generators
- **core:** filter auto-imports by script runtime

## [0.1.6](https://github.com/instar-luau/instar/compare/v0.1.5...v0.1.6) - 2026-10-25

### Added

- **assets:** import Creator Docs documentation

### Fixed

- **bridge:** resolve relative imports from script instances
- isolate filesystem and Roblox module resolution
- **bridge:** unify Roblox vector type identities
- **bridge:** use authoritative module identities for semantic queries
- **bridge:** expand synthetic table names in type presentations

### Other

- **assets:** satisfy Creator Docs importer lints
- **core:** format import generation
- **bridge:** avoid temporary strings in builtin names

## [0.1.5](https://github.com/instar-luau/instar/compare/v0.1.4...v0.1.5) - 2026-10-24

### Added

- **cli:** associate published schema without generation

## [0.1.4](https://github.com/instar-luau/instar/compare/v0.1.3...v0.1.4) - 2026-10-23

### Added

- **assets:** import authoritative Luau documentation

### Fixed

- **server:** skip source-less documentation fallbacks

### Other

- apply boolean naming prefixes

## [0.1.3](https://github.com/instar-luau/instar/compare/v0.1.2...v0.1.3) - 2026-10-23

### Fixed

- resolve declaration documentation across editor features
- **bridge:** compact composite inlay hint types
- **bridge:** preserve named types in bounded presentations

### Other

- expand readme and document configuration

## [0.1.2](https://github.com/instar-luau/instar/compare/v0.1.1...v0.1.2) - 2026-10-23

### Fixed

- **bridge:** collapse oversized anonymous table hints
- **bridge:** clarify enum and overload hover presentation

### Other

- **server:** resolve document links on demand
- **core:** share workspace dependency observations
- **bridge:** reuse occurrence and workspace indexes
- **bridge:** check dependency-ready modules in parallel
- **server:** prioritize interactive editor requests

## [0.1.1](https://github.com/instar-luau/instar/compare/v0.1.0...v0.1.1) - 2026-10-22

### Other

- update Cargo.lock dependencies
- update Cargo.toml dependencies

## [0.1.0] - 2026-10-23

### Added

- Initial release.
