#ifndef INSTAR_FRONTEND_STATE_HEADER
#define INSTAR_FRONTEND_STATE_HEADER

#include "../frontend.hpp"
#include "Luau/ConfigResolver.h"
#include "Luau/Frontend.h"
#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <tuple>
#include <vector>

namespace instar {
    namespace frontend {
        struct Resolver final : Luau::FileResolver, Luau::ConfigResolver {
            const Host *host = nullptr;
            std::string active;
            std::string activeSource;
            std::map<std::string, Luau::Config> configurations;
            std::map<std::string, std::string> environments;

            std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override;

            std::optional<Luau::ModuleInfo>
            resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override;

            std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName &name) const override;
            const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override;
        };
    } // namespace frontend

    struct NativeFrontend::State {
        frontend::Resolver resolver;
        Luau::Frontend frontend{Luau::SolverMode::New, &resolver, &resolver};

        using Signature = std::pair<
            std::vector<std::tuple<std::string, uint64_t, std::string>>,
            std::vector<std::tuple<std::string, bool, bool, std::vector<std::tuple<std::string, bool, bool>>>>>;

        std::map<std::string, Signature> definitions;
        uint64_t environmentRevision = 0;

        State();
    };
} // namespace instar

#endif
