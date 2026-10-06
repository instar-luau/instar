#include "state.hpp"
#include "diagnostics.hpp"
#include "instar-bridge/src/boundary.rs.h"

#include "Luau/Ast.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/TypeArena.h"
#include <stdexcept>

namespace instar::frontend {
    std::optional<Luau::SourceCode> Resolver::readSource(const Luau::ModuleName &name) {
        if (!host) {
            throw std::runtime_error("host callbacks are unavailable");
        }

        const NativeSource source = host->read_source(name);

        if (!source.found) {
            return std::nullopt;
        }

        active = name;
        activeSource = std::string(source.text);

        return Luau::SourceCode{std::string(source.text), Luau::SourceCode::Module};
    }

    std::optional<Luau::ModuleInfo>
    Resolver::resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) {
        if (!host) {
            throw std::runtime_error("host callbacks are unavailable");
        }

        const rust::String target = host->resolve(
            active,
            offset(activeSource, expression->location.begin),
            offset(activeSource, expression->location.end)
        );

        if (target.empty()) {
            return std::nullopt;
        }

        return Luau::ModuleInfo{std::string(target)};
    }

    std::optional<std::string> Resolver::getEnvironmentForModule(const Luau::ModuleName &name) const {
        const auto found = environments.find(name);

        if (found == environments.end()) {
            return std::nullopt;
        }

        return found->second;
    }

    const Luau::Config &Resolver::getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const {
        const auto found = configurations.find(name);

        if (found == configurations.end()) {
            throw std::runtime_error("host configuration is unavailable: " + name);
        }

        return found->second;
    }
} // namespace instar::frontend

namespace instar {
    NativeFrontend::State::State() {
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
        Luau::freeze(frontend.globals.globalTypes);
    }
} // namespace instar
