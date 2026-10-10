#include "frontend.hpp"
#include "frontend/checking.hpp"
#include "frontend/documentation.hpp"
#include "frontend/hierarchy.hpp"
#include "frontend/hover.hpp"
#include "frontend/linting.hpp"
#include "frontend/navigation.hpp"
#include "frontend/parsing.hpp"
#include "frontend/references.hpp"
#include "frontend/signatures.hpp"
#include "frontend/state.hpp"
#include "frontend/suggestions.hpp"

#include "instar-bridge/src/boundary.rs.h"

namespace instar {
    namespace {
        std::string text(rust::Str value) {
            return std::string(value.data(), value.size());
        }

    }

    NativeFrontend::NativeFrontend() : state(std::make_unique<frontend::State>()) {}

    NativeFrontend::~NativeFrontend() = default;

    void NativeFrontend::configure(rust::Str name, const NativeConfiguration &configuration) {
        const std::string module = text(name);
        state->frontend.markDirty(module);
        state->resolver.configurations.insert_or_assign(module, configuration.value());
    }

    rust::Vec<NativeLink> NativeFrontend::prepare(const Host &host, rust::Slice<const rust::String> names) {
        return frontend::prepare(*state, host, names);
    }

    void NativeFrontend::invalidate(rust::Slice<const rust::String> names) {
        if (names.empty()) {
            return;
        }

        std::vector<Luau::ModuleName> modules;

        for (const rust::String &name : names) {
            modules.emplace_back(name);
            state->resolver.configurations.erase(std::string(name));
            state->definitions.erase(std::string(name));
        }

        state->frontend.clearModules(modules);
        state->frontend.clearBuiltinEnvironments();
        state->resolver.environments.clear();
        state->definitions.clear();
        state->environments.clear();
        state->locations.clear();
        state->lints.clear();

        for (const auto &[name, node] : state->frontend.sourceNodes) {
            state->frontend.markDirty(name);
        }
    }

    rust::String NativeFrontend::documentation(rust::Str module, rust::Str symbol) const {
        return frontend::documentation(*state, module, symbol);
    }

    void NativeFrontend::retain() {
        if (!state->frontend.options.retainFullTypeGraphs) {
            state->frontend.options.retainFullTypeGraphs = true;

            for (const auto &[name, node] : state->frontend.sourceNodes) {
                state->frontend.markDirty(name);
            }
        }
    }

    NativeHover NativeFrontend::hover(const Host &host, rust::Str module, uint32_t line, uint32_t column) {
        return frontend::hover(*state, host, module, line, column);
    }

    rust::Vec<NativeLocation> NativeFrontend::definition(
        const Host &host, rust::Str module, uint32_t line, uint32_t column, bool type_definition
    ) {
        return frontend::definition(*state, host, module, line, column, type_definition);
    }

    NativeSuggestions NativeFrontend::suggest(const Host &host, rust::Str module, uint32_t line, uint32_t column) {
        return frontend::suggest(*state, host, module, line, column);
    }

    NativeSignatures NativeFrontend::signatures(const Host &host, rust::Str module, uint32_t line, uint32_t column) {
        return frontend::signatures(*state, host, module, line, column);
    }

    rust::Vec<NativeReference> NativeFrontend::references(const Host &host, rust::Slice<const rust::String> modules) {
        return frontend::references(*state, host, modules);
    }

    NativeHierarchy NativeFrontend::hierarchy(
        const Host &host, rust::Slice<const rust::String> modules, rust::Str module,
        rust::Slice<const uint32_t> position
    ) {
        return frontend::hierarchy(*state, host, modules, module, position);
    }

    NativeCheck NativeFrontend::check(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation
    ) {
        bool prepared = false;

        return frontend::analyze(*state, host, entries, timeout_seconds, names, cancellation, true, false, prepared);
    }

    NativeLintResult NativeFrontend::lint(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation,
        rust::Slice<const rust::String> semantic_modules
    ) {
        return frontend::lint(*state, host, entries, timeout_seconds, names, cancellation, semantic_modules);
    }

    std::unique_ptr<NativeFrontend> create_frontend() {
        return std::make_unique<NativeFrontend>();
    }
}
