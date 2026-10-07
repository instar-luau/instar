#include "diagnostics.hpp"
#include "state.hpp"

#include "Luau/Ast.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/TypeArena.h"
#include "instar-bridge/src/boundary.rs.h"

#include <stdexcept>
#include <tuple>

namespace instar::frontend {
    std::optional<Luau::SourceCode> Resolver::readSource(const Luau::ModuleName &name) {
        if (!host) {
            throw std::runtime_error("host callbacks are unavailable");
        }

        const NativeDocument source = host->read_document(name);

        if (!source.found) {
            return std::nullopt;
        }

        active = name;

        return Luau::SourceCode{std::string(source.text), Luau::SourceCode::Module};
    }

    std::optional<Luau::ModuleInfo>
    Resolver::resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) {
        if (!host) {
            throw std::runtime_error("host callbacks are unavailable");
        }

        const rust::String target = host->resolve(
            active,
            offset(*host, active, expression->location.begin),
            offset(*host, active, expression->location.end)
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

    bool Definition::operator==(const Definition &other) const {
        return name == other.name && revision == other.revision && namespace_name == other.namespace_name;
    }

    bool Definition::operator<(const Definition &other) const {
        return std::tie(name, revision, namespace_name) < std::tie(other.name, other.revision, other.namespace_name);
    }

    bool Property::operator==(const Property &other) const {
        return name == other.name && read == other.read && write == other.write;
    }

    bool Property::operator<(const Property &other) const {
        return std::tie(name, read, write) < std::tie(other.name, other.read, other.write);
    }

    bool Class::operator==(const Class &other) const {
        return name == other.name && service == other.service && creatable == other.creatable &&
               properties == other.properties;
    }

    bool Class::operator<(const Class &other) const {
        return std::tie(name, service, creatable, properties) <
               std::tie(other.name, other.service, other.creatable, other.properties);
    }

    bool Signature::operator==(const Signature &other) const {
        return definitions == other.definitions && classes == other.classes;
    }

    bool Signature::operator<(const Signature &other) const {
        return std::tie(definitions, classes) < std::tie(other.definitions, other.classes);
    }

    State::State() {
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
        Luau::freeze(frontend.globals.globalTypes);
    }
}
