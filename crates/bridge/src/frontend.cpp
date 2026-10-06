#include "instar-bridge/src/lib.rs.h"

#include "Luau/Ast.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Cancellation.h"
#include "Luau/ConfigResolver.h"
#include "Luau/Error.h"
#include "Luau/Frontend.h"
#include "Luau/Module.h"
#include "Luau/TypeArena.h"

#include <atomic>
#include <chrono>
#include <exception>
#include <map>
#include <stdexcept>
#include <string>
#include <thread>
#include <tuple>
#include <utility>
#include <vector>

namespace instar {
    namespace {
        std::string text(rust::Str value) {
            return std::string(value.data(), value.size());
        }

        size_t offset(const std::string &source, Luau::Position position) {
            size_t start = 0;

            for (unsigned int line = 0; line < position.line; ++line) {
                const size_t newline = source.find('\n', start);

                if (newline == std::string::npos) {
                    throw std::runtime_error("native source position exceeds host revision");
                }

                start = newline + 1;
            }

            if (position.column > source.size() - start) {
                throw std::runtime_error("native source column exceeds host revision");
            }

            return start + position.column;
        }

        struct Resolver final : Luau::FileResolver, Luau::ConfigResolver {
            const Host *host = nullptr;
            std::string active;
            std::string activeSource;
            std::map<std::string, Luau::Config> configurations;
            std::map<std::string, std::string> environments;

            std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override {
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
            resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override {
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

            std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName &name) const override {
                const auto found = environments.find(name);

                if (found == environments.end()) {
                    return std::nullopt;
                }

                return found->second;
            }

            const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override {
                const auto found = configurations.find(name);

                if (found == configurations.end()) {
                    throw std::runtime_error("host configuration is unavailable: " + name);
                }

                return found->second;
            }
        };

        struct Calls final : Luau::AstVisitor {
            const std::string &source;
            std::map<std::pair<size_t, size_t>, Luau::AstExprCall *> calls;

            explicit Calls(const std::string &source) : source(source) {}

            bool visit(Luau::AstExprCall *call) override {
                calls.emplace(
                    std::make_pair(offset(source, call->location.begin), offset(source, call->location.end)),
                    call
                );

                return true;
            }

            bool visit(Luau::AstType *) override { return true; }

            bool visit(Luau::AstTypePack *) override { return true; }
        };

        NativeLocation location(const Host &host, const Luau::ModuleName &name, Luau::Location range) {
            const NativeSource source = host.read_source(name);

            if (!source.found) {
                throw std::runtime_error("native diagnostic source is unavailable: " + name);
            }

            const std::string bytes(source.text);

            return NativeLocation{rust::String(name),
                source.revision,
                offset(bytes, range.begin),
                offset(bytes, range.end)};
        }

        NativeKind kind(const Luau::TypeError &error) {
            if (Luau::get<Luau::SyntaxError>(error)) {
                return NativeKind::Syntax;
            }

            if (Luau::get<Luau::UnknownRequire>(error) || Luau::get<Luau::IllegalRequire>(error)) {
                return NativeKind::Resolution;
            }

            if (Luau::get<Luau::CodeTooComplex>(error) || Luau::get<Luau::UnificationTooComplex>(error) ||
                Luau::get<Luau::NormalizationTooComplex>(error) ||
                Luau::get<Luau::ConstraintSolvingIncompleteError>(error) || Luau::get<Luau::InternalError>(error)) {
                return NativeKind::Analysis;
            }

            return NativeKind::Type;
        }

        NativeDiagnostic diagnostic(const Host &host, const Luau::TypeError &error, const std::string &fallback) {
            const std::string name = error.moduleName.empty() ? fallback : error.moduleName;

            NativeDiagnostic result{location(host, name, error.location),
                kind(error),
                error.code(),
                rust::String(Luau::toString(error)),
                {}};

            const auto *count = Luau::get<Luau::CountMismatch>(error);

            if (Luau::get<Luau::GenericError>(error) ||
                (count && count->context == Luau::CountMismatch::Arg && count->expected == 1 && count->actual != 1)) {
                const NativeSource source = host.read_source(name);

                for (const NativeSite &site : source.sites) {
                    if (site.call_start == result.location.start && site.call_end == result.location.end) {
                        result.kind = NativeKind::Resolution;
                        break;
                    }
                }
            }

            if (const auto *duplicate = Luau::get<Luau::DuplicateTypeDefinition>(error);
                duplicate && duplicate->previousLocation) {
                result.related.push_back(
                    NativeRelated{location(host, name, *duplicate->previousLocation),
                        rust::String("Previous type definition")}
                );
            }

            const Luau::TypeError *nested = &error;

            while (const auto *mismatch = Luau::get<Luau::TypeMismatch>(*nested)) {
                if (!mismatch->error) {
                    break;
                }

                nested = mismatch->error.get();
                const std::string relatedName = nested->moduleName.empty() ? name : nested->moduleName;

                result.related.push_back(
                    NativeRelated{location(host, relatedName, nested->location), rust::String(Luau::toString(*nested))}
                );
            }

            return result;
        }

        struct Interruption {
            const Cancellation &cancellation;
            std::chrono::steady_clock::time_point started = std::chrono::steady_clock::now();
            double seconds;

            std::shared_ptr<Luau::FrontendCancellationToken> token =
                std::make_shared<Luau::FrontendCancellationToken>();

            std::atomic<bool> stopped{false};
            std::thread monitor;

            Interruption(const Cancellation &cancellation, double seconds)
                : cancellation(cancellation), seconds(seconds) {
                token->cancelled.store(false);

                monitor = std::thread([this] {
                    while (!stopped.load()) {
                        if (status() != NativeCompletion::Complete) {
                            token->cancel();
                            return;
                        }
                        std::this_thread::sleep_for(std::chrono::milliseconds(1));
                    }
                });
            }

            ~Interruption() {
                stopped.store(true);
                monitor.join();
            }

            double remaining() const {
                return seconds - std::chrono::duration<double>(std::chrono::steady_clock::now() - started).count();
            }

            NativeCompletion status() const {
                if (cancellation.requested()) {
                    return NativeCompletion::Cancelled;
                }

                return remaining() <= 0 ? NativeCompletion::Timeout : NativeCompletion::Complete;
            }
        };
    } // namespace

    struct NativeFrontend::State {
        Resolver resolver;
        Luau::Frontend frontend{Luau::SolverMode::New, &resolver, &resolver};
        std::map<std::string, std::vector<std::tuple<std::string, uint64_t, std::string>>> definitions;
        uint64_t environmentRevision = 0;

        State() {
            Luau::registerBuiltinGlobals(frontend, frontend.globals);
            Luau::freeze(frontend.globals.globalTypes);
        }
    };

    NativeFrontend::NativeFrontend() : state(std::make_unique<State>()) {}

    NativeFrontend::~NativeFrontend() = default;

    std::unique_ptr<NativeFrontend> create_frontend() {
        return std::make_unique<NativeFrontend>();
    }

    void NativeFrontend::configure(rust::Str name, const NativeConfiguration &configuration) {
        const std::string module = text(name);
        state->frontend.markDirty(module);
        state->resolver.configurations.insert_or_assign(module, configuration.value());
    }

    rust::Vec<NativeLink> NativeFrontend::prepare(const Host &host, rust::Slice<const rust::String> names) {
        auto &frontend = state->frontend;
        state->resolver.host = &host;

        struct Reset {
            Resolver &resolver;
            ~Reset() { resolver.host = nullptr; }
        } reset{state->resolver};

        std::vector<Luau::ModuleName> modules;

        for (const rust::String &name : names) {
            modules.emplace_back(name);
        }

        frontend.parseModules(modules);

        rust::Vec<NativeLink> links;

        for (const std::string &name : modules) {
            const NativeSource source = host.read_source(name);
            Luau::SourceModule *module = frontend.getSourceModule(name);
            const auto node = frontend.sourceNodes.find(name);

            if (!source.found || !module || !module->root || node == frontend.sourceNodes.end()) {
                throw std::runtime_error("native module source is unavailable: " + name);
            }

            const std::string bytes(source.text);
            Calls calls(bytes);
            module->root->visit(&calls);
            Luau::RequireTraceResult trace;
            node->second->requireSet.clear();
            node->second->requireLocations.clear();

            for (const NativeSite &site : source.sites) {
                const auto found = calls.calls.find({site.call_start, site.call_end});

                if (found == calls.calls.end()) {
                    if (site.static_request && module->parseErrors.empty()) {
                        throw std::runtime_error("host/native static require call disagreement: " + name);
                    }

                    continue;
                }

                Luau::AstExprCall *call = found->second;

                if (site.static_request &&
                    (call->args.size != 1 || offset(bytes, call->args.data[0]->location.begin) != site.argument_start ||
                        offset(bytes, call->args.data[0]->location.end) != site.argument_end)) {
                    throw std::runtime_error("host/native static require argument disagreement: " + name);
                }

                const std::string target(site.target);

                if (target.empty()) {
                    trace.exprs[call] = {};
                    continue;
                }

                if (frontend.sourceNodes.find(target) == frontend.sourceNodes.end()) {
                    throw std::runtime_error("host target was not parsed: " + target);
                }

                trace.exprs[call->args.data[0]] = Luau::ModuleInfo{target};
                trace.exprs[call] = Luau::ModuleInfo{target};
                trace.requireList.emplace_back(target, call->location);
                node->second->requireSet.insert(target);
                node->second->requireLocations.emplace_back(target, call->location);

                links.push_back(
                    NativeLink{rust::String(name),
                        source.revision,
                        site.call_start,
                        site.call_end,
                        site.argument_start,
                        site.argument_end,
                        rust::String(target)}
                );
            }

            frontend.requireTrace.insert_or_assign(name, std::move(trace));
        }

        for (const auto &entry : frontend.sourceNodes) {
            entry.second->dependents.clear();
        }

        for (const auto &[name, node] : frontend.sourceNodes) {
            for (const std::string &target : node->requireSet) {
                if (const auto found = frontend.sourceNodes.find(target); found != frontend.sourceNodes.end()) {
                    found->second->dependents.insert(name);
                }
            }
        }

        return links;
    }

    NativeCheck NativeFrontend::check(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation
    ) {
        auto &frontend = state->frontend;
        Interruption interruption(cancellation, timeout_seconds);
        NativeCheck result{{}, interruption.status()};

        if (result.completion != NativeCompletion::Complete || entries.empty()) {
            return result;
        }

        struct DirtyIncomplete {
            State &state;
            NativeCheck &result;

            ~DirtyIncomplete() {
                if (result.completion != NativeCompletion::Complete || std::uncaught_exceptions() != 0) {
                    state.frontend.clearBuiltinEnvironments();
                    state.resolver.environments.clear();
                    state.definitions.clear();

                    for (const auto &[name, node] : state.frontend.sourceNodes) {
                        state.frontend.markDirty(name);
                    }
                }
            }
        } dirty{*state, result};

        try {
            for (const rust::String &opaque : names) {
                const std::string name(opaque);
                const NativeSource source = host.read_source(name);
                std::vector<std::tuple<std::string, uint64_t, std::string>> signature;

                for (const NativeDefinition &definition : source.definitions) {
                    signature
                        .emplace_back(std::string(definition.name), definition.revision, std::string(definition.text));
                }

                if (const auto found = state->definitions.find(name);
                    found != state->definitions.end() && found->second == signature) {
                    continue;
                }

                frontend.markDirty(name);

                if (source.definitions.empty()) {
                    state->resolver.environments.erase(name);
                    state->definitions.insert_or_assign(name, std::move(signature));
                    continue;
                }

                bool reused = false;

                for (const auto &[existing, definitions] : state->definitions) {
                    if (definitions == signature) {
                        const auto environment = state->resolver.environments.find(existing);

                        if (environment != state->resolver.environments.end()) {
                            state->resolver.environments.insert_or_assign(name, environment->second);
                            reused = true;
                            break;
                        }
                    }
                }

                if (reused) {
                    state->definitions.insert_or_assign(name, std::move(signature));
                    continue;
                }

                result.completion = interruption.status();

                if (result.completion != NativeCompletion::Complete) {
                    return result;
                }

                const std::string environment = "environment:" + std::to_string(++state->environmentRevision);
                Luau::ScopePtr scope = frontend.addEnvironment(environment);

                {
                    struct Restore {
                        Luau::GlobalTypes &globals;
                        Luau::ScopePtr scope;

                        ~Restore() {
                            globals.globalScope = scope;
                            Luau::freeze(globals.globalTypes);
                        }
                    } restore{frontend.globals, frontend.globals.globalScope};

                    Luau::unfreeze(frontend.globals.globalTypes);
                    frontend.globals.globalScope = scope;

                    for (const NativeDefinition &definition : source.definitions) {
                        result.completion = interruption.status();

                        if (result.completion != NativeCompletion::Complete) {
                            return result;
                        }

                        const std::string definitionName(definition.name);

                        const auto loaded = frontend.loadDefinitionFile(
                            frontend.globals,
                            scope,
                            std::string(definition.text),
                            definitionName,
                            false
                        );

                        for (const Luau::ParseError &error : loaded.parseResult.errors) {
                            const Luau::TypeError syntax{error.getLocation(),
                                definitionName,
                                Luau::SyntaxError{error.what()}};

                            result.diagnostics.push_back(diagnostic(host, syntax, definitionName));
                        }

                        if (loaded.module) {
                            for (const Luau::TypeError &error : loaded.module->errors) {
                                result.diagnostics.push_back(diagnostic(host, error, definitionName));
                            }
                        }

                        result.completion = interruption.status();

                        if (result.completion != NativeCompletion::Complete) {
                            return result;
                        }

                        if (!loaded.success) {
                            result.completion = NativeCompletion::Environment;

                            return result;
                        }
                    }
                }

                state->resolver.environments.insert_or_assign(name, environment);
                state->definitions.insert_or_assign(name, std::move(signature));
            }

            result.completion = interruption.status();

            if (result.completion != NativeCompletion::Complete) {
                return result;
            }

            prepare(host, names);
            result.completion = interruption.status();

            if (result.completion != NativeCompletion::Complete) {
                return result;
            }

            state->resolver.host = &host;

            struct Reset {
                Resolver &resolver;
                ~Reset() { resolver.host = nullptr; }
            } reset{state->resolver};

            for (const rust::String &entry : entries) {
                result.completion = interruption.status();

                if (result.completion != NativeCompletion::Complete) {
                    return result;
                }

                Luau::FrontendOptions options;
                options.cancellationToken = interruption.token;
                options.moduleTimeLimitSec = interruption.remaining();
                const Luau::CheckResult checked = frontend.check(std::string(entry), options);

                if (!checked.timeoutHits.empty()) {
                    result.completion = NativeCompletion::Timeout;
                    break;
                }
            }

            for (const rust::String &opaque : names) {
                const std::string name(opaque);
                const auto module = frontend.moduleResolver.getModule(name);

                if (module && module->cancelled) {
                    result.completion =
                        cancellation.requested() ? NativeCompletion::Cancelled : NativeCompletion::Timeout;

                    continue;
                }

                const auto checked = frontend.getCheckResult(name, false);

                if (!checked) {
                    if (result.completion == NativeCompletion::Complete) {
                        result.completion = NativeCompletion::Analysis;
                    }

                    continue;
                }

                if (!checked->timeoutHits.empty()) {
                    result.completion = NativeCompletion::Timeout;
                }

                const Luau::Config &configuration = state->resolver.getConfig(name, {});

                for (const Luau::TypeError &error : checked->errors) {
                    if (const auto *illegal = Luau::get<Luau::IllegalRequire>(error)) {
                        const auto *dependency = frontend.getSourceModule(illegal->moduleName);

                        if (dependency && !dependency->parseErrors.empty()) {
                            continue;
                        }
                    }

                    const NativeKind category = kind(error);

                    if (category == NativeKind::Type && !configuration.typeErrors) {
                        continue;
                    }

                    if (category == NativeKind::Analysis && result.completion == NativeCompletion::Complete) {
                        result.completion = NativeCompletion::Analysis;
                    }

                    result.diagnostics.push_back(diagnostic(host, error, name));
                }
            }
        } catch (const Luau::UserCancelError &) {
            result.completion = NativeCompletion::Cancelled;
        } catch (const Luau::TimeLimitError &) {
            result.completion = NativeCompletion::Timeout;
        } catch (const Luau::InternalCompilerError &error) {
            const std::string name = error.moduleName.value_or(std::string(entries[0]));

            const Luau::TypeError failure{error.location.value_or(Luau::Location{}),
                name,
                Luau::InternalError{error.what()}};

            result.diagnostics.push_back(diagnostic(host, failure, name));
            result.completion = NativeCompletion::Analysis;
        }

        if (const NativeCompletion status = interruption.status(); status != NativeCompletion::Complete) {
            result.completion = status;
        }

        return result;
    }

    void NativeFrontend::invalidate(rust::Slice<const rust::String> names) {
        if (names.empty()) {
            return;
        }

        std::vector<Luau::ModuleName> modules;

        for (const rust::String &name : names) {
            modules.emplace_back(name);
            state->resolver.configurations.erase(std::string(name));
            state->definitions.erase(std::string(name));
        }

        state->frontend.clearModules(modules);
        state->frontend.clearBuiltinEnvironments();
        state->resolver.environments.clear();
        state->definitions.clear();

        for (const auto &[name, node] : state->frontend.sourceNodes) {
            state->frontend.markDirty(name);
        }
    }
} // namespace instar
