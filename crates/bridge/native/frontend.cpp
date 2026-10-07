#include "frontend.hpp"
#include "frontend/checking.hpp"
#include "frontend/documentation.hpp"
#include "frontend/linting.hpp"
#include "frontend/parsing.hpp"
#include "frontend/state.hpp"

#include "instar-bridge/src/boundary.rs.h"

namespace instar {
    namespace {
        std::string text(rust::Str value) {
            return std::string(value.data(), value.size());
        }

    } // namespace

    NativeFrontend::NativeFrontend() : state(std::make_unique<frontend::State>()) {}

    NativeFrontend::~NativeFrontend() = default;

    std::unique_ptr<NativeFrontend> create_frontend() {
        return std::make_unique<NativeFrontend>();
    }

    void NativeFrontend::configure(rust::Str name, const NativeConfiguration &configuration) {
        const std::string module = text(name);
        state->frontend.markDirty(module);
        state->resolver.configurations.insert_or_assign(module, configuration.value());
    }

    rust::Vec<NativeLink> NativeFrontend::prepare(const Host &host, rust::Slice<const rust::String> names) {
        return frontend::prepare(*state, host, names);
    }

    NativeCheck NativeFrontend::check(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation
    ) {
        return frontend::analyze(*state, host, entries, timeout_seconds, names, cancellation, true);
    }

    NativeLintResult NativeFrontend::lint(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation,
        rust::Slice<const rust::String> semantic_modules
    ) {
        return frontend::lint(*state, host, entries, timeout_seconds, names, cancellation, semantic_modules);
    }

    rust::String NativeFrontend::documentation(rust::Str module, rust::Str symbol) const {
        return frontend::documentation(*state, module, symbol);
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

        for (const auto &[name, node] : state->frontend.sourceNodes) {
            state->frontend.markDirty(name);
        }
    }
} // namespace instar
