#ifndef INSTAR_ANALYSIS_HEADER
#define INSTAR_ANALYSIS_HEADER

#include "Luau/ConfigResolver.h"
#include "Luau/Frontend.h"
#include "instar-luau/src/boundary/mod.rs.h"

#include <cstdint>
#include <map>
#include <optional>
#include <string>

namespace instar::native {
    struct Resolver final : Luau::FileResolver, Luau::ConfigResolver {
        const NativeOperation *host = nullptr;
        std::string active;
        std::map<std::string, Luau::Config> configurations;
        std::map<std::string, std::string> environments;
        std::map<std::string, NativeModuleInput> captured;

        std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override;

        std::optional<Luau::ModuleInfo>
        resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override;

        std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName &name) const override;
        const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override;
    };

    struct ModuleOutcome {
        rust::Vec<NativeDiagnostic> prerequisites;
        bool environment_available = false;
        bool parsed = false;
        bool inferred = false;
        bool graph_available = false;
    };

    struct AnalysisState {
        Resolver resolver;
        Luau::Frontend frontend{Luau::SolverMode::New, &resolver, &resolver};
        std::map<std::string, std::string> environments;
        std::map<std::string, std::map<std::string, NativeLocation>> locations;
        uint64_t environment_revision = 0;

        uint64_t revision = 0;
        uint8_t purpose = 0;
        NativeAnalysisRequest captured;
        std::map<std::string, ModuleOutcome> outcomes;
        bool reusable = false;
        AnalysisState();
    };

    NativeAnalysisResult
    analyze(AnalysisState &state, const NativeAnalysisRequest &input, const NativeOperation &operation);

    void validate(
        const AnalysisState &state, uint64_t frontend_revision, const std::string &name, uint64_t logical_revision,
        bool inference, uint8_t purpose
    );
}

#endif
