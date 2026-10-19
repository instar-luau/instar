#include "frontend.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Parser.h"
#include "Luau/RequireTracer.h"
#include "Luau/TypeCheckLimits.h"
#include "cancellation.hpp"
#include "diagnostic.hpp"
#include "environment.hpp"
#include "index.hpp"
#include "instar-bridge/src/bridge.rs.h"
#include "query.hpp"

#include <algorithm>
#include <set>
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

            if (interrupted || options.cancellationToken->requested()) {
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

    std::uint64_t Frontend::cancellation() const {
        return retain(options.cancellationToken);
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
            auto diagnostics = instar::luau::prepare(*frontend, name, host.environment(name));
            found = environments.emplace(name, std::move(diagnostics)).first;
        }

        if (!found->second.empty()) {
            throw DeclarationFailure{found->second};
        }
    }

    const Instances *Frontend::hierarchy(const std::string &name) {
        return host.prepare(*frontend, name, {});
    }

    rust::Vec<Hover> Frontend::hover(rust::Str module, std::uint32_t line, std::uint32_t column) {
        rust::Vec<Hover> result;

        for (const auto &name : prepare(module)) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto *source = frontend->getSourceModule(name);
            const auto text = host.readSource(name);

            if (!text) {
                throw std::runtime_error("semantic source is unavailable");
            }

            for (auto &value : query::hover(*frontend, host, {*checked, *source, text->source}, {line, column})) {
                result.push_back(std::move(value));
            }
        }

        return result;
    }

    rust::Vec<Completion> Frontend::completion(rust::Str module, std::uint32_t line, std::uint32_t column) {
        rust::Vec<Completion> result;

        for (const auto &name : prepare(module)) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto *source = frontend->getSourceModule(name);
            const auto text = host.readSource(name);

            if (!text) {
                throw std::runtime_error("semantic source is unavailable");
            }

            for (auto &value : query::completion(*frontend, host, {*checked, *source, text->source}, {line, column})) {
                result.push_back(std::move(value));
            }
        }

        return result;
    }

    rust::Vec<Signature> Frontend::signatures(rust::Str module, std::uint32_t line, std::uint32_t column) {
        rust::Vec<Signature> result;

        for (const auto &name : prepare(module)) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto *source = frontend->getSourceModule(name);
            const auto text = host.readSource(name);

            if (!text) {
                throw std::runtime_error("semantic source is unavailable");
            }

            for (auto &value : query::signatures(host, {*checked, *source, text->source}, {line, column})) {
                result.push_back(std::move(value));
            }
        }

        return result;
    }

    rust::Vec<SourceLocation>
    Frontend::definitions(rust::Str module, std::uint32_t line, std::uint32_t column, bool types) {
        rust::Vec<SourceLocation> result;

        for (const auto &name : prepare(module)) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto *source = frontend->getSourceModule(name);
            const auto text = host.readSource(name);

            if (!text) {
                throw std::runtime_error("semantic source is unavailable");
            }

            for (auto &value :
                query::definitions(*frontend, host, {*checked, *source, text->source}, {line, column}, types)) {
                result.push_back(std::move(value));
            }
        }

        return result;
    }

    rust::Vec<Occurrence> Frontend::occurrences(rust::Str module) {
        rust::Vec<Occurrence> result;

        for (const auto &name : prepare(module)) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto indexed = index(*frontend, *checked, *frontend->getSourceModule(name));

            for (const auto &entry : indexed.entries) {
                result.push_back(query::occurrence(host, name, entry));
            }
        }

        return result;
    }

    rust::Vec<Occurrence> Frontend::rename(
        rust::Str module, std::uint32_t line, std::uint32_t column, rust::Str replacement,
        rust::Slice<const rust::String> candidates
    ) {
        const std::string name(replacement);
        query::identifier(name);
        std::vector<Entry> targets;
        std::set<std::string> sources{std::string(module)};

        for (const auto &candidate : candidates) {
            sources.emplace(std::string(candidate));
        }

        for (const auto &identity : prepare(module)) {
            const auto checked = frontend->moduleResolver.getModule(identity);
            const auto *source = frontend->getSourceModule(identity);
            const auto text = host.readSource(identity);

            if (!text) {
                throw std::runtime_error("semantic source is unavailable");
            }

            auto entry = query::target(*frontend, {*checked, *source, text->source}, {line, column});

            if (!targets.empty()) {
                const auto &previous = targets.front();

                if (previous.kind != entry.kind || previous.name != entry.name ||
                    previous.type_namespace != entry.type_namespace) {
                    throw std::invalid_argument("rename target differs between instance contexts");
                }

                if (host.getHumanReadableModuleName(previous.declaration.identity) !=
                        host.getHumanReadableModuleName(entry.declaration.identity) ||
                    previous.declaration.location != entry.declaration.location) {
                    throw std::invalid_argument("rename declaration differs between instance contexts");
                }
            }

            sources.insert(host.getHumanReadableModuleName(entry.declaration.identity));
            targets.push_back(std::move(entry));
        }

        rust::Vec<Occurrence> result;

        if (targets.empty()) {
            return result;
        }

        std::vector<bool> declared(targets.size(), false);

        for (const auto &source : sources) {
            for (const auto &identity : prepare(source)) {
                const auto checked = frontend->moduleResolver.getModule(identity);
                const auto *syntax = frontend->getSourceModule(identity);
                const auto text = host.readSource(identity);

                if (!text) {
                    throw std::runtime_error("semantic source is unavailable");
                }

                for (auto &value :
                    query::rename(*frontend, host, {*checked, *syntax, text->source}, name, targets, declared)) {
                    result.push_back(std::move(value));
                }
            }
        }

        if (std::find(declared.begin(), declared.end(), false) != declared.end()) {
            throw std::invalid_argument("native rename validation requires an editable source declaration");
        }

        return result;
    }

    rust::Vec<Hint> Frontend::hints(rust::Str module) {
        rust::Vec<Hint> result;

        for (const auto &name : prepare(module)) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto *source = frontend->getSourceModule(name);

            for (auto &value : query::hints(host, *checked, *source)) {
                result.push_back(std::move(value));
            }
        }

        return result;
    }

    rust::Vec<Import> Frontend::imports(rust::Str module) {
        try {
            rust::Vec<Import> result;
            Luau::TypeCheckLimits limits;
            limits.cancellationToken = options.cancellationToken;

            if (options.cancellationToken->requested()) {
                throw std::runtime_error("analysis interrupted");
            }

            for (const auto &name : sourceIdentities(module)) {
                if (options.cancellationToken->requested() ||
                    !callbacks.progress(host.getHumanReadableModuleName(name), false)) {
                    throw std::runtime_error("analysis interrupted");
                }

                const auto text = host.readSource(name);

                if (!text) {
                    throw std::runtime_error("import source is unavailable");
                }

                Luau::SourceModule source;

                const auto parsed = Luau::Parser::parse(
                    text->source.data(),
                    text->source.size(),
                    *source.names,
                    *source.allocator,
                    host.getConfig(name, limits).parseOptions
                );

                if (options.cancellationToken->requested()) {
                    throw std::runtime_error("analysis interrupted");
                }

                if (parsed.root) {
                    const auto trace = Luau::traceRequires(&host, parsed.root, name, limits);

                    for (auto &value : query::imports(host, name, trace)) {
                        result.push_back(std::move(value));
                    }
                }

                if (options.cancellationToken->requested() ||
                    !callbacks.progress(host.getHumanReadableModuleName(name), true)) {
                    throw std::runtime_error("analysis interrupted");
                }
            }

            return result;
        } catch (const DeclarationFailure &failure) {
            const auto message =
                failure.diagnostics.empty() ? "import resolution failed" : std::string(failure.diagnostics[0].message);

            throw std::runtime_error(message);
        }
    }

    void Frontend::initialize() {
        auto initialization = options;
        initialization.cancellationToken.reset();
        auto initialized = std::make_unique<Luau::Frontend>(Luau::SolverMode::New, &host, &host, initialization);
        Luau::registerBuiltinGlobals(*initialized, initialized->globals);
        Luau::freeze(initialized->globals.globalTypes);
        initialized->options = options;

        initialized->prepareModuleScope = [this](const Luau::ModuleName &name, const Luau::ScopePtr &scope, bool) {
            if (!callbacks.progress(host.getHumanReadableModuleName(name), false)) {
                throw std::runtime_error("analysis interrupted");
            }

            host.prepare(*frontend, name, scope);
        };

        frontend = std::move(initialized);
    }

    std::vector<std::string> Frontend::sourceIdentities(rust::Str module) {
        const std::string physical(module);
        auto names = host.modules(physical);

        if (names.size() > 1 && std::find(names.begin(), names.end(), physical) == names.end()) {
            names.push_back(physical);
        }

        for (const auto &[identity, source] : frontend->sourceModules) {
            if (host.getHumanReadableModuleName(identity) == physical &&
                std::find(names.begin(), names.end(), identity) == names.end()) {
                names.push_back(identity);
            }
        }

        return names;
    }

    std::vector<std::string> Frontend::prepare(rust::Str module) {
        if (!options.retainFullTypeGraphs) {
            throw std::runtime_error("semantic queries require retained full type graphs");
        }

        const auto names = sourceIdentities(module);

        frontend->queueModuleCheck(names);
        const auto diagnostics = analyze(module);

        for (const auto &name : names) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto *source = frontend->getSourceModule(name);

            if (!checked || !source || !source->root || frontend->isDirty(name)) {
                const auto message =
                    diagnostics.empty() ? "semantic module is unavailable" : std::string(diagnostics[0].message);

                throw std::runtime_error(message);
            }

            if (checked->cancelled || options.cancellationToken->requested()) {
                throw std::runtime_error("analysis interrupted");
            }

            if (checked->timeout) {
                throw std::runtime_error("semantic analysis timed out");
            }
        }

        return names;
    }

    std::unique_ptr<Frontend> create(const Context &host, bool retain_types) {
        const auto flags = host.flags();
        Luau::FrontendOptions options;
        options.retainFullTypeGraphs = retain_types;
        options.cancellationToken = std::make_shared<Luau::FrontendCancellationToken>();
        options.cancellationToken->cancelled.store(false);

        return std::make_unique<Frontend>(host, rust::Slice<const Flag>{flags.data(), flags.size()}, options);
    }
}
