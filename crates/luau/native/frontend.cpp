#include "frontend.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Error.h"
#include "Luau/TypeArena.h"
#include "diagnostic.hpp"
#include "environment.hpp"
#include "instar-luau/src/bridge.rs.h"

#include <optional>
#include <stdexcept>
#include <utility>

namespace instar::luau {
    Frontend::Frontend(const Context &context, rust::Slice<const Flag> settings, const Luau::FrontendOptions &options)
        : flags(settings), host(context, flags, *this), options(options) {
        initialize();
    }

    void Frontend::initialize() {
        auto initialized = std::make_unique<Luau::Frontend>(Luau::SolverMode::New, &host, &host, options);
        Luau::registerBuiltinGlobals(*initialized, initialized->globals);
        Luau::freeze(initialized->globals.globalTypes);

        initialized->prepareModuleScope = [this](const Luau::ModuleName &name, const Luau::ScopePtr &scope, bool) {
            host.prepare(*frontend, name, scope);
        };

        frontend = std::move(initialized);
    }

    rust::Vec<Diagnostic> Frontend::analyze(rust::Str module) {
        try {
            rust::Vec<Diagnostic> diagnostics;

            for (const std::string &name : host.modules(std::string(module))) {
                std::optional<Luau::CheckResult> result = frontend->getCheckResult(name, true);

                if (!result) {
                    frontend->queueModuleCheck(name);

                    for (const std::string &checked_name : frontend->checkQueuedModules()) {
                        if (const auto checked = frontend->moduleResolver.getModule(checked_name)) {
                            attribute(*checked);
                        }
                    }

                    result = frontend->getCheckResult(name, true);
                }

                if (!result) {
                    throw std::runtime_error("Module not found: " + host.getHumanReadableModuleName(name));
                }

                if (!result->timeoutHits.empty()) {
                    throw std::runtime_error(
                        "Luau analysis timed out: " + host.getHumanReadableModuleName(result->timeoutHits.front())
                    );
                }

                for (const Luau::TypeError &error : result->errors) {
                    diagnostics.push_back(diagnose(error, host));
                }
            }

            return diagnostics;
        } catch (const DeclarationFailure &failure) {
            return failure.diagnostics;
        } catch (...) {
            clear();
            throw;
        }
    }

    void Frontend::invalidate(rust::Str module) {
        const std::string name(module);

        for (const std::string &identity : host.modules(name)) {
            frontend->markDirty(identity);
            host.invalidate(identity);
        }

        frontend->markDirty(name);
        host.invalidate(name);
    }

    void Frontend::clear() {
        initialize();
        environments.clear();
        host.clear();
    }

    void Frontend::environment(const std::string &name) {
        auto found = environments.find(name);

        if (found == environments.end()) {
            auto diagnostics = prepare(*frontend, name, host.environment(name));
            found = environments.emplace(name, std::move(diagnostics)).first;
        }

        if (!found->second.empty()) {
            throw DeclarationFailure{found->second};
        }
    }

    const Instances *Frontend::hierarchy(const std::string &name) {
        return host.prepare(*frontend, name, {});
    }

    std::unique_ptr<Frontend> create(const Context &host, bool retainTypes) {
        const auto flags = host.flags();
        Luau::FrontendOptions options;
        options.retainFullTypeGraphs = retainTypes;

        return std::make_unique<Frontend>(host, rust::Slice<const Flag>{flags.data(), flags.size()}, options);
    }
}
