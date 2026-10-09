#include "checking.hpp"
#include "diagnostics.hpp"
#include "documentation.hpp"
#include "linting.hpp"
#include "navigation.hpp"
#include "parsing.hpp"
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
#include <vector>

namespace instar::frontend {
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

    }

    NativeCheck analyze(
        State &state, const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation, bool typecheck, bool lint,
        bool &graph_prepared
    ) {
        graph_prepared = false;
        auto &frontend = state.frontend;
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
                    state.environments.clear();
                    state.lints.clear();

                    for (const auto &[name, node] : state.frontend.sourceNodes) {
                        state.frontend.markDirty(name);
                    }
                }
            }
        } dirty{state, result};

        try {
            size_t prepared = 0;

            for (const rust::String &opaque : names) {
                const std::string name(opaque);
                host.progress("Loading declarations", name, prepared++, names.size());
                const NativeSource source = host.read_source(name);
                Signature signature;

                for (const NativeDefinition &definition : source.definitions) {
                    signature.definitions.push_back(
                        Definition{std::string(definition.name),
                            definition.revision,
                            std::string(definition.namespace_name)}
                    );
                }

                for (const NativeClass &klass : source.classes) {
                    std::vector<Property> properties;

                    for (const NativeProperty &property : klass.properties) {
                        properties.push_back(Property{std::string(property.name), property.read, property.write});
                    }

                    signature.classes.push_back(
                        Class{std::string(klass.name), klass.service, klass.creatable, std::move(properties)}
                    );
                }

                if (const auto found = state.definitions.find(name);
                    found != state.definitions.end() && *found->second == signature) {
                    continue;
                }

                frontend.markDirty(name);

                if (const auto found = state.environments.find(signature); found != state.environments.end()) {
                    state.definitions.insert_or_assign(name, &found->first);

                    if (found->second.empty()) {
                        state.resolver.environments.erase(name);
                    } else {
                        state.resolver.environments.insert_or_assign(name, found->second);
                    }

                    continue;
                }

                if (source.definitions.empty()) {
                    const auto inserted = state.environments.emplace(std::move(signature), std::string{});
                    state.definitions.insert_or_assign(name, &inserted.first->first);
                    state.resolver.environments.erase(name);
                    continue;
                }

                result.completion = interruption.status();

                if (result.completion != NativeCompletion::Complete) {
                    return result;
                }

                const std::string environment = "environment:" + std::to_string(++state.environment_revision);
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

                        const std::string definition_name(definition.name);
                        const NativeDocument document = host.read_document(definition_name);

                        if (!document.found || document.revision != definition.revision) {
                            throw std::runtime_error("native declaration revision is unavailable: " + definition_name);
                        }

                        const auto loaded = frontend.loadDefinitionFile(
                            frontend.globals,
                            scope,
                            std::string(document.text),
                            definition_name,
                            false
                        );

                        for (const Luau::ParseError &error : loaded.parseResult.errors) {
                            const Luau::TypeError syntax{error.getLocation(),
                                definition_name,
                                Luau::SyntaxError{error.what()}};

                            result.diagnostics.push_back(diagnostic(host, syntax, definition_name));
                        }

                        if (loaded.module) {
                            for (const Luau::TypeError &error : loaded.module->errors) {
                                result.diagnostics.push_back(diagnostic(host, error, definition_name));
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

                        register_declarations(state, host, *loaded.parseResult.root, definition_name, environment);
                        assign_documentation(scope, definition_name, std::string(definition.namespace_name));
                    }

                    if (!source.classes.empty()) {
                        register_roblox_magic(
                            frontend.globals,
                            rust::Slice<const NativeClass>{source.classes.data(), source.classes.size()}
                        );
                    }
                }

                state.resolver.environments.insert_or_assign(name, environment);
                const auto inserted = state.environments.emplace(std::move(signature), environment);
                state.definitions.insert_or_assign(name, &inserted.first->first);
            }

            host.progress("Loading declarations", "", prepared, names.size());

            result.completion = interruption.status();

            if (result.completion != NativeCompletion::Complete) {
                return result;
            }

            for (const rust::String &name : names) {
                if (lint && state.lints.count(std::string(name)) == 0) {
                    frontend.markDirty(std::string(name));
                }
            }

            for (const rust::String &name : names) {
                if (frontend.isDirty(std::string(name))) {
                    state.lints.erase(std::string(name));
                }
            }

            prepare(state, host, names);
            graph_prepared = true;
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

            state.resolver.host = &host;

            struct Reset {
                Resolver &resolver;

                ~Reset() { resolver.host = nullptr; }
            } reset{state.resolver};

            size_t completed = 0;
            size_t total = 0;

            for (const rust::String &name : names) {
                if (frontend.isDirty(std::string(name))) {
                    ++total;
                }
            }

            host.progress("Checking", "", 0, total);

            for (const rust::String &entry : entries) {
                frontend.queueModuleCheck(std::string(entry));
            }

            Luau::FrontendOptions options = frontend.options;
            options.cancellationToken = interruption.token;
            options.moduleTimeLimitSec = interruption.remaining();

            options.customModuleCheck = [&](const Luau::SourceModule &source, const Luau::Module &module) {
                if (!module.cancelled) {
                    try {
                        if (lint) {
                            collect(state, host, source, module);
                        }
                        host.progress("Checking", source.name, ++completed, total);
                    } catch (const std::exception &error) {
                        throw Luau::InternalCompilerError(error.what(), source.name);
                    }
                }
            };

            frontend.checkQueuedModules(options, {}, [&](size_t, size_t) {
                return interruption.status() == NativeCompletion::Complete;
            });

            result.completion = interruption.status();

            for (const rust::String &opaque : names) {
                const std::string name(opaque);
                const auto module = frontend.moduleResolver.getModule(name);

                if (module && module->cancelled) {
                    result.completion =
                        cancellation.requested() ? NativeCompletion::Cancelled : NativeCompletion::Timeout;

                    continue;
                }

                if (lint && module && !module->timeout && state.lints.count(name) == 0) {
                    const auto *source = frontend.getSourceModule(name);

                    if (source && source->root) {
                        collect(state, host, *source, *module);
                    }
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

                const Luau::Config &configuration = state.resolver.getConfig(name, {});

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
}
