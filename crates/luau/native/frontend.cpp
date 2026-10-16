#include "frontend.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Error.h"
#include "Luau/TypeArena.h"
#include "diagnostic.hpp"
#include "environment.hpp"
#include "instar-luau/src/bridge.rs.h"
#include "linting.hpp"

#include <optional>
#include <stdexcept>
#include <utility>

namespace instar::luau {
    Frontend::Frontend(const Context &context, rust::Slice<const Flag> settings, const Luau::FrontendOptions &options)
        : flags(settings), host(context, flags, *this), options(options) {
        initialize();
    }

    void Frontend::initialize() {
        options.customModuleCheck = [this](const Luau::SourceModule &source, const Luau::Module &module) {
            if (linting) {
                findings[source.name] = lint(source, module, host, frontend->globals.globalScope);
            }
        };

        auto initialized = std::make_unique<Luau::Frontend>(Luau::SolverMode::New, &host, &host, options);
        Luau::registerBuiltinGlobals(*initialized, initialized->globals);
        Luau::freeze(initialized->globals.globalTypes);

        initialized->prepareModuleScope = [this](const Luau::ModuleName &name, const Luau::ScopePtr &scope, bool) {
            host.prepare(*frontend, name, scope);
        };

        frontend = std::move(initialized);
    }

    Analysis Frontend::analyze(rust::Str module, bool lint) {
        if (linting != lint) {
            linting = lint;
            clear();
        }

        try {
            Analysis analysis;

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
                    analysis.diagnostics.push_back(diagnose(error, host));

                    if (const auto *syntax = Luau::get<Luau::SyntaxError>(error);
                        lint && error.moduleName == name && syntax &&
                        syntax->message.find("malformed escape sequence") != std::string::npos) {
                        analysis.warnings.push_back(
                            Warning{analysis.diagnostics[analysis.diagnostics.size() - 1],
                                "bad_string_escape",
                                {},
                                {},
                                false}
                        );
                    }
                }

                if (const auto found = findings.find(name); found != findings.end()) {
                    for (const auto &warning : found->second) {
                        analysis.warnings.push_back(warning);
                    }
                }
            }

            return analysis;
        } catch (const DeclarationFailure &failure) {
            Analysis analysis;
            analysis.diagnostics = failure.diagnostics;

            for (auto &diagnostic : analysis.diagnostics) {
                diagnostic.blocking = true;
            }

            return analysis;
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
        findings.clear();
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

    rust::Vec<rust::String> warnings() {
        rust::Vec<rust::String> names;

        for (int code = 1; code < Luau::LintWarning::Code__Count; ++code) {
            names.push_back(Luau::LintWarning::getName(static_cast<Luau::LintWarning::Code>(code)));
        }

        return names;
    }
}
