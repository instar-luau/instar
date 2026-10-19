#include "frontend.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Error.h"
#include "Luau/RequireTracer.h"
#include "Luau/TypeArena.h"
#include "diagnostic.hpp"
#include "environment.hpp"
#include "instar-bridge/src/bridge.rs.h"

#include <stdexcept>
#include <utility>
#include <vector>

namespace instar::luau {
    Frontend::Frontend(const Context &context, rust::Slice<const Flag> settings, const Luau::FrontendOptions &options)
        : callbacks(context), flags(settings), host(context, flags, *this), options(options) {
        initialize();
    }

    rust::Vec<Diagnostic> Frontend::analyze(rust::Str module) {
        try {
            rust::Vec<Diagnostic> diagnostics;
            const auto names = host.modules(std::string(module));
            frontend->queueModuleCheck(names);
            bool interrupted = false;

            const auto checked_names =
                frontend->checkQueuedModules({}, {}, [&](std::size_t completed, std::size_t total) {
                    interrupted = !callbacks.advance(completed, total);
                    return !interrupted;
                });

            if (interrupted) {
                throw std::runtime_error("analysis interrupted");
            }

            for (const std::string &checked_name : checked_names) {
                if (const auto checked = frontend->moduleResolver.getModule(checked_name)) {
                    attribute(*checked);
                }

                if (!callbacks.progress(host.getHumanReadableModuleName(checked_name), true)) {
                    throw std::runtime_error("analysis interrupted");
                }
            }

            for (const std::string &name : names) {
                const auto result = frontend->getCheckResult(name, true);

                if (!result) {
                    throw std::runtime_error("cannot find module `" + host.getHumanReadableModuleName(name) + "`");
                }

                if (!result->timeoutHits.empty()) {
                    throw std::runtime_error(
                        "analysis timed out for `" + host.getHumanReadableModuleName(result->timeoutHits.front()) + "`"
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

    void Frontend::queue(rust::Str module) {
        try {
            frontend->queueModuleCheck(host.modules(std::string(module)));
        } catch (...) {
            clear();
            throw;
        }
    }

    std::size_t Frontend::checks() const {
        return frontend->stats.filesStrict + frontend->stats.filesNonstrict;
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

    void Frontend::refresh() {
        frontend->moduleQueue.clear();
        std::vector<std::string> changed;

        for (const auto &[name, source] : frontend->sourceModules) {
            if (!source->root || frontend->isDirty(name)) {
                continue;
            }

            const auto current = Luau::traceRequires(&host, source->root, name, {});
            const auto previous = frontend->requireTrace.find(name);

            if (previous == frontend->requireTrace.end() || current.requireList != previous->second.requireList) {
                changed.push_back(name);
            }
        }

        for (const auto &name : changed) {
            frontend->markDirty(name);
            host.invalidate(name);
        }
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

    void Frontend::initialize() {
        auto initialized = std::make_unique<Luau::Frontend>(Luau::SolverMode::New, &host, &host, options);
        Luau::registerBuiltinGlobals(*initialized, initialized->globals);
        Luau::freeze(initialized->globals.globalTypes);

        initialized->prepareModuleScope = [this](const Luau::ModuleName &name, const Luau::ScopePtr &scope, bool) {
            if (!callbacks.progress(host.getHumanReadableModuleName(name), false)) {
                throw std::runtime_error("analysis interrupted");
            }

            host.prepare(*frontend, name, scope);
        };

        frontend = std::move(initialized);
    }

    std::unique_ptr<Frontend> create(const Context &host, bool retain_types) {
        const auto flags = host.flags();
        Luau::FrontendOptions options;
        options.retainFullTypeGraphs = retain_types;

        return std::make_unique<Frontend>(host, rust::Slice<const Flag>{flags.data(), flags.size()}, options);
    }
}
