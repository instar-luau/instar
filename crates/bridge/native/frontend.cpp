#include "frontend.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Common.h"
#include "Luau/Parser.h"
#include "Luau/RequireTracer.h"
#include "Luau/TypeCheckLimits.h"
#include "diagnostic.hpp"
#include "environment.hpp"
#include "hover.hpp"
#include "index.hpp"
#include "instar-bridge/src/bridge.rs.h"
#include "query.hpp"
#include "types.hpp"

#include <algorithm>
#include <atomic>
#include <functional>
#include <future>
#include <set>
#include <stdexcept>
#include <thread>
#include <utility>
#include <vector>

LUAU_FASTFLAG(LuauSplitIceHandler)
LUAU_FASTFLAG(DebugLuauLogSolverToJson)

namespace instar::luau {
    namespace {
        struct CheckedModule {
            Luau::ModulePtr module;
            const Luau::SourceModule &source;
        };

        class Checking {
        public:
            explicit Checking(Luau::Frontend &frontend)
                : frontend(frontend), prepare(frontend.prepareModuleScope), pending(frontend.moduleQueue) {
                frontend.prepareModuleScope = [this](const Luau::ModuleName &name, const Luau::ScopePtr &scope, bool) {
                    for (const auto &[symbol, binding] : std::as_const(scopes).at(name)->bindings) {
                        scope->bindings.insert_or_assign(symbol, binding);
                    }
                };
            }

            ~Checking() {
                frontend.prepareModuleScope = std::move(prepare);
            }

            Checking(const Checking &) = delete;
            Checking &operator=(const Checking &) = delete;

            void execute(std::vector<std::function<void()>> tasks) {
                while (!pending.empty()) {
                    auto name = std::move(pending.back());
                    pending.pop_back();

                    if (scopes.count(name) || !frontend.isDirty(name)) {
                        continue;
                    }

                    const auto source = frontend.sourceNodes.find(name);

                    if (source == frontend.sourceNodes.end()) {
                        continue;
                    }

                    auto scope = std::make_shared<Luau::Scope>(nullptr);
                    prepare(name, scope, false);
                    scopes.emplace(name, std::move(scope));
                    pending.insert(pending.end(), source->second->requireSet.begin(), source->second->requireSet.end());
                }

                const auto count =
                    FFlag::LuauSplitIceHandler && !FFlag::DebugLuauLogSolverToJson
                        ? std::min<std::size_t>(tasks.size(), std::max(1u, std::thread::hardware_concurrency()))
                        : 1;

                std::atomic<std::size_t> next{0};

                auto perform = [&] {
                    while (true) {
                        const auto position = next.fetch_add(1, std::memory_order_relaxed);

                        if (position >= tasks.size()) {
                            return;
                        }

                        tasks[position]();
                    }
                };

                std::vector<std::future<void>> workers;
                workers.reserve(count);

                for (std::size_t index = 1; index < count; ++index) {
                    workers.push_back(std::async(std::launch::async, perform));
                }

                perform();

                for (auto &worker : workers) {
                    worker.get();
                }
            }

        private:
            Luau::Frontend &frontend;
            decltype(Luau::Frontend::prepareModuleScope) prepare;
            std::vector<std::string> pending;
            std::unordered_map<std::string, Luau::ScopePtr> scopes;
        };

        auto declaration(const SourceLocation &source, bool is_type_namespace) {
            const auto &range = source.range;

            return std::make_tuple(
                std::string(source.module),
                std::string(source.identity),
                Luau::Position{range.start.line, range.start.column},
                Luau::Position{range.end.line, range.end.column},
                is_type_namespace
            );
        }

        CheckedModule checked_module(Luau::Frontend &frontend, const std::string &name) {
            const auto module = frontend.moduleResolver.getModule(name);
            const auto *source = frontend.getSourceModule(name);

            return {module, *source};
        }

        std::string semantic_text(Host &host, const std::string &name) {
            auto text = host.readSource(name);

            if (!text) {
                throw std::runtime_error("Source is unavailable for the semantic query.");
            }

            return std::move(text->source);
        }
    }

    Frontend::Frontend(const Context &context, rust::Slice<const Flag> settings, const Luau::FrontendOptions &options)
        : callbacks(context), flags(settings), host(context, flags, *this), options(options) {
        initialize();
    }

    Frontend::~Frontend() = default;

    rust::Vec<Diagnostic> Frontend::check(rust::Str module) {
        try {
            rust::Vec<Diagnostic> diagnostics;
            const auto names = host.modules(std::string(module));
            frontend->queueModuleCheck(names);
            bool is_interrupted = false;

            Checking checking(*frontend);

            const auto checked_names = frontend->checkQueuedModules(
                {},
                [&](std::vector<std::function<void()>> tasks) { checking.execute(std::move(tasks)); },
                [&](std::size_t completed, std::size_t total) {
                    is_interrupted = !callbacks.advance(completed, total);
                    return !is_interrupted;
                }
            );

            if (is_interrupted || options.cancellationToken->requested()) {
                throw std::runtime_error("Analysis interrupted.");
            }

            for (const std::string &checked_name : checked_names) {
                if (const auto checked = frontend->moduleResolver.getModule(checked_name)) {
                    attribute(*checked);
                }

                if (!callbacks.progress(host.getHumanReadableModuleName(checked_name), true)) {
                    throw std::runtime_error("Analysis interrupted.");
                }
            }

            for (const std::string &name : names) {
                const auto result = frontend->getCheckResult(name, true);

                if (!result) {
                    throw std::runtime_error(
                        "Analysis result is unavailable for module `" + host.getHumanReadableModuleName(name) + "`."
                    );
                }

                if (!result->timeoutHits.empty()) {
                    throw std::runtime_error(
                        "Analysis timed out for `" + host.getHumanReadableModuleName(result->timeoutHits.front()) + "`."
                    );
                }

                for (const Luau::TypeError &error : result->errors) {
                    if (host.getConfig(error.moduleName, {}).typeErrors || Luau::get<Luau::SyntaxError>(error)) {
                        diagnostics.push_back(diagnose(error, host));
                    }
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
            dirty(identity);
        }

        dirty(name);
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
            dirty(name);
        }
    }

    void Frontend::clear() {
        initialize();
        environments.clear();
        host.clear();
        indexes.clear();
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
        return host.instances(*frontend, name);
    }

    rust::Vec<NativeHover> Frontend::hover(rust::Str module, std::uint32_t line, std::uint32_t column) {
        rust::Vec<NativeHover> result;
        PresentationBudget budget{options.cancellationToken};

        auto names = prepare(module);
        std::sort(names.begin(), names.end());

        for (const auto &name : names) {
            const auto [checked, source] = checked_module(*frontend, name);
            const auto text = semantic_text(host, name);

            for (auto &value : query::hover(*frontend, host, {*checked, source, text}, {line, column}, budget)) {
                result.push_back(std::move(value));
            }
        }

        budget.check();

        return result;
    }

    rust::Vec<NativeCompletion> Frontend::completion(rust::Str module, std::uint32_t line, std::uint32_t column) {
        return complete(module, line, column, std::nullopt);
    }

    rust::Vec<NativeCompletion>
    Frontend::completion_detail(rust::Str module, std::uint32_t line, std::uint32_t column, std::size_t index) {
        return complete(module, line, column, index);
    }

    rust::Vec<NativeSignature> Frontend::signatures(rust::Str module, std::uint32_t line, std::uint32_t column) {
        rust::Vec<NativeSignature> result;
        PresentationBudget budget{options.cancellationToken};

        auto names = prepare(module);
        std::sort(names.begin(), names.end());

        for (const auto &name : names) {
            const auto [checked, source] = checked_module(*frontend, name);
            const auto text = semantic_text(host, name);

            for (auto &value : query::signatures(*frontend, host, {*checked, source, text}, {line, column}, budget)) {
                result.push_back(std::move(value));
            }
        }

        budget.check();

        return result;
    }

    rust::Vec<SourceLocation>
    Frontend::definitions(rust::Str module, std::uint32_t line, std::uint32_t column, bool should_resolve_types) {
        rust::Vec<SourceLocation> result;

        for (const auto &name : prepare(module)) {
            const auto [checked, source] = checked_module(*frontend, name);
            const auto text = semantic_text(host, name);

            for (auto &value :
                query::definitions(*frontend, host, {*checked, source, text}, {line, column}, should_resolve_types)) {
                result.push_back(std::move(value));
            }
        }

        return result;
    }

    rust::Vec<Occurrence> Frontend::occurrences(rust::Str module, rust::Slice<const Occurrence> declarations) {
        rust::Vec<Occurrence> result;

        for (const auto &name : host.modules(std::string(module))) {
            if (options.cancellationToken->requested()) {
                throw std::runtime_error("Analysis interrupted.");
            }

            auto found = indexes.find(name);

            if (found == indexes.end()) {
                prepare(module);
                const auto [checked, source] = checked_module(*frontend, name);
                Occurrences occurrences{index(*frontend, *checked, source).entries, {}};

                for (std::size_t position = 0; position < occurrences.entries.size(); ++position) {
                    const auto &entry = occurrences.entries[position];
                    const auto location = query::locate(host, entry.declaration.identity, entry.declaration.location);
                    occurrences.declarations[declaration(location, entry.is_type_namespace)].push_back(position);
                }

                found = indexes.try_emplace(name).first;
                found->second.entries.swap(occurrences.entries);
                found->second.declarations.swap(occurrences.declarations);
            }

            if (declarations.empty()) {
                for (const auto &entry : found->second.entries) {
                    result.push_back(query::occurrence(host, name, entry));
                }
            } else {
                for (const auto &target : declarations) {
                    const auto selected =
                        found->second.declarations.find(declaration(target.declaration, target.is_type_namespace));

                    if (selected != found->second.declarations.end()) {
                        for (const auto position : selected->second) {
                            result.push_back(query::occurrence(host, name, found->second.entries[position]));
                        }
                    }
                }
            }
        }

        return result;
    }

    rust::Vec<Occurrence> Frontend::rename(
        rust::Str module, std::uint32_t line, std::uint32_t column, rust::Str replacement,
        rust::Slice<const rust::String> candidates
    ) {
        const std::string name(replacement);
        query::validate_identifier(name);
        std::vector<Entry> targets;
        std::set<std::string> sources{std::string(module)};

        for (const auto &candidate : candidates) {
            sources.emplace(std::string(candidate));
        }

        for (const auto &identity : prepare(module)) {
            const auto [checked, source] = checked_module(*frontend, identity);
            const auto text = semantic_text(host, identity);

            auto entry = query::target(*frontend, {*checked, source, text}, {line, column});

            if (!targets.empty()) {
                const auto &previous = targets.front();

                if (previous.kind != entry.kind || previous.name != entry.name ||
                    previous.is_type_namespace != entry.is_type_namespace) {
                    throw std::invalid_argument("Rename target differs between instance contexts.");
                }

                if (host.getHumanReadableModuleName(previous.declaration.identity) !=
                        host.getHumanReadableModuleName(entry.declaration.identity) ||
                    previous.declaration.location != entry.declaration.location) {
                    throw std::invalid_argument("Rename declaration differs between instance contexts.");
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
                const auto [checked, syntax] = checked_module(*frontend, identity);
                const auto text = semantic_text(host, identity);

                for (auto &value : query::rename(*frontend, host, {*checked, syntax, text}, name, targets, declared)) {
                    result.push_back(std::move(value));
                }
            }
        }

        if (std::find(declared.begin(), declared.end(), false) != declared.end()) {
            throw std::invalid_argument("Native rename validation requires an editable source declaration.");
        }

        return result;
    }

    rust::Vec<NativeHint> Frontend::hints_range(rust::Str module, Position start, Position end, bool is_expanded) {
        rust::Vec<NativeHint> result;
        PresentationBudget budget{options.cancellationToken};

        auto names = prepare(module);
        std::sort(names.begin(), names.end());

        for (const auto &name : names) {
            const auto [checked, source] = checked_module(*frontend, name);

            for (auto &value : query::hints(
                     *frontend,
                     host,
                     *checked,
                     source,
                     budget,
                     is_expanded,
                     {start.line, start.column},
                     {end.line, end.column}
                 )) {
                result.push_back(std::move(value));
            }
        }

        budget.check();

        return result;
    }

    rust::Vec<NativeHint> Frontend::hint_subjects(rust::Slice<const HintSelector> selectors) {
        rust::Vec<NativeHint> result;
        PresentationBudget budget{options.cancellationToken};

        for (const auto &selector : selectors) {
            budget.check();
            const std::string identity(selector.identity);

            for (const auto &name : prepare(identity)) {
                if (name != identity) {
                    continue;
                }

                const auto [checked, source] = checked_module(*frontend, name);
                const Luau::Position position{selector.position.line, selector.position.column};

                for (auto &value :
                    query::hints(*frontend, host, *checked, source, budget, true, position, position, selector.kind)) {
                    result.push_back(std::move(value));
                }
            }
        }

        budget.check();

        return result;
    }

    rust::Vec<Import> Frontend::imports(rust::Str module) {
        try {
            rust::Vec<Import> result;
            Luau::TypeCheckLimits limits;
            limits.cancellationToken = options.cancellationToken;

            if (options.cancellationToken->requested()) {
                throw std::runtime_error("Analysis interrupted.");
            }

            for (const auto &name : host.modules(std::string(module))) {
                if (options.cancellationToken->requested() ||
                    !callbacks.progress(host.getHumanReadableModuleName(name), false)) {
                    throw std::runtime_error("Analysis interrupted.");
                }

                const auto cached = frontend->requireTrace.find(name);

                if (cached != frontend->requireTrace.end() && !frontend->isDirty(name)) {
                    for (auto &value : query::imports(host, name, cached->second)) {
                        result.push_back(std::move(value));
                    }
                } else {
                    const auto text = host.readSource(name);

                    if (!text) {
                        throw std::runtime_error("Source is unavailable for import resolution.");
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
                        throw std::runtime_error("Analysis interrupted.");
                    }

                    if (parsed.root) {
                        const auto trace = Luau::traceRequires(&host, parsed.root, name, limits);

                        for (auto &value : query::imports(host, name, trace)) {
                            result.push_back(std::move(value));
                        }
                    }
                }

                if (options.cancellationToken->requested() ||
                    !callbacks.progress(host.getHumanReadableModuleName(name), true)) {
                    throw std::runtime_error("Analysis interrupted.");
                }
            }

            return result;
        } catch (const DeclarationFailure &failure) {
            const auto message =
                failure.diagnostics.empty() ? "Import resolution failed." : std::string(failure.diagnostics[0].message);

            throw std::runtime_error(message);
        }
    }

    std::shared_ptr<Luau::FrontendCancellationToken> Frontend::cancellation() const {
        return options.cancellationToken;
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
                throw std::runtime_error("Analysis interrupted.");
            }

            host.bind(*frontend, name, scope);
        };

        frontend = std::move(initialized);
    }

    void Frontend::dirty(const std::string &name) {
        std::vector<std::string> changed;
        frontend->markDirty(name, &changed);
        host.invalidate(name);

        for (const auto &identity : changed) {
            indexes.erase(identity);
        }
    }

    std::vector<std::string> Frontend::prepare(rust::Str module) {
        if (!options.retainFullTypeGraphs) {
            throw std::runtime_error("Semantic queries require retained full type graphs.");
        }

        const auto names = host.modules(std::string(module));

        frontend->queueModuleCheck(names);
        const auto diagnostics = check(module);

        for (const auto &name : names) {
            const auto checked = frontend->moduleResolver.getModule(name);
            const auto *source = frontend->getSourceModule(name);

            if (!checked || !source || !source->root || frontend->isDirty(name)) {
                const auto message = diagnostics.empty() ? "Module is unavailable for the semantic query."
                                                         : std::string(diagnostics[0].message);

                throw std::runtime_error(message);
            }

            if (checked->cancelled || options.cancellationToken->requested()) {
                throw std::runtime_error("Analysis interrupted.");
            }

            if (checked->timeout) {
                throw std::runtime_error("Semantic analysis timed out.");
            }
        }

        return names;
    }

    rust::Vec<NativeCompletion> Frontend::complete(
        rust::Str module, std::uint32_t line, std::uint32_t column, std::optional<std::size_t> selected
    ) {
        rust::Vec<NativeCompletion> result;
        PresentationBudget budget{options.cancellationToken};
        std::size_t candidate_index = 0;
        std::vector<CompletionType> types;

        auto names = prepare(module);
        std::sort(names.begin(), names.end());

        for (const auto &name : names) {
            const auto [checked, source] = checked_module(*frontend, name);
            const auto text = semantic_text(host, name);

            for (auto &value : query::completion(
                     *frontend,
                     host,
                     {*checked, source, text},
                     {line, column},
                     budget,
                     types,
                     candidate_index,
                     selected
                 )) {
                result.push_back(std::move(value));
            }
        }

        budget.check();

        return result;
    }

    std::unique_ptr<Frontend> create(const Context &host, bool should_retain_types) {
        const auto flags = host.flags();
        Luau::FrontendOptions options;
        options.retainFullTypeGraphs = should_retain_types;
        options.cancellationToken = std::make_shared<Luau::FrontendCancellationToken>();
        options.cancellationToken->cancelled.store(false);

        return std::make_unique<Frontend>(host, rust::Slice<const Flag>{flags.data(), flags.size()}, options);
    }
}
