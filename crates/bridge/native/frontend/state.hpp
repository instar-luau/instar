#ifndef INSTAR_FRONTEND_STATE_HEADER
#define INSTAR_FRONTEND_STATE_HEADER

#include "Luau/ConfigResolver.h"
#include "Luau/Frontend.h"

#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <vector>

namespace instar {
    struct Host;
}

namespace instar::frontend {
    struct Resolver final : Luau::FileResolver, Luau::ConfigResolver {
        const Host *host = nullptr;
        std::string active;
        std::string active_source;
        std::map<std::string, Luau::Config> configurations;
        std::map<std::string, std::string> environments;

        std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override;

        std::optional<Luau::ModuleInfo>
        resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override;

        std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName &name) const override;
        const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override;
    };

    struct Definition {
        std::string name;
        uint64_t revision;
        std::string text;
        std::string namespace_name;

        bool operator==(const Definition &other) const;
    };

    struct Property {
        std::string name;
        bool read;
        bool write;

        bool operator==(const Property &other) const;
    };

    struct Class {
        std::string name;
        bool service;
        bool creatable;
        std::vector<Property> properties;

        bool operator==(const Class &other) const;
    };

    struct Signature {
        std::vector<Definition> definitions;
        std::vector<Class> classes;

        bool operator==(const Signature &other) const;
    };

    struct State {
        Resolver resolver;
        Luau::Frontend frontend{Luau::SolverMode::New, &resolver, &resolver};
        std::map<std::string, Signature> definitions;
        uint64_t environment_revision = 0;

        State();
    };
}

#endif
