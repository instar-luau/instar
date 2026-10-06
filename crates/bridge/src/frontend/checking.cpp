#include "instar-bridge/src/boundary.rs.h"

#include "diagnostics.hpp"
#include "roblox.hpp"
#include "state.hpp"

#include "Luau/Cancellation.h"
#include "Luau/Error.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/TypeArena.h"

#include <atomic>
#include <chrono>
#include <exception>
#include <thread>
#include <utility>

namespace instar {
    using frontend::diagnostic;
    using frontend::kind;
    using frontend::Resolver;

    namespace {
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

    NativeCheck NativeFrontend::check(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation
    ) {
        return analyze(host, entries, timeout_seconds, names, cancellation, true);
    }

    NativeCheck NativeFrontend::analyze(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation, bool typecheck
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
                State::Signature signature;

                for (const NativeDefinition &definition : source.definitions) {
                    signature.first
                        .emplace_back(std::string(definition.name), definition.revision, std::string(definition.text));
                }

                for (const NativeClass &klass : source.classes) {
                    std::vector<std::tuple<std::string, bool, bool>> properties;

                    for (const NativeProperty &property : klass.properties) {
                        properties.emplace_back(std::string(property.name), property.read, property.write);
                    }

                    signature.second
                        .emplace_back(std::string(klass.name), klass.service, klass.creatable, std::move(properties));
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

                    if (!source.classes.empty()) {
                        register_roblox_magic(
                            frontend.globals,
                            rust::Slice<const NativeClass>{source.classes.data(), source.classes.size()}
                        );
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

            if (!typecheck) {
                for (const rust::String &opaque : names) {
                    const std::string name(opaque);
                    const auto *source = frontend.getSourceModule(name);

                    for (const Luau::ParseError &error : source->parseErrors) {
                        const Luau::TypeError syntax{error.getLocation(), name, Luau::SyntaxError{error.what()}};
                        result.diagnostics.push_back(diagnostic(host, syntax, name));
                    }
                }

                result.completion = interruption.status();

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
                options.retainFullTypeGraphs = true;
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

} // namespace instar
