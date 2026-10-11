#include "analysis.hpp"
#include "environment.hpp"
#include "parsing.hpp"

#include "Luau/Ast.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Cancellation.h"
#include "Luau/Common.h"
#include "Luau/Error.h"
#include "Luau/Module.h"
#include "Luau/TypeArena.h"

#include <atomic>
#include <chrono>
#include <limits>
#include <stdexcept>
#include <string_view>
#include <thread>
#include <vector>

namespace instar::native {
    namespace {
        struct Interruption {
            const NativeOperation &operation;

            std::shared_ptr<Luau::FrontendCancellationToken> token =
                std::make_shared<Luau::FrontendCancellationToken>();

            std::atomic<bool> stopped{false};
            std::thread monitor;

            explicit Interruption(const NativeOperation &operation) : operation(operation) {
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

            NativeCompletion status() const {
                if (operation.requested()) {
                    return NativeCompletion::Cancelled;
                }

                return operation.remaining_nanoseconds() == 0 ? NativeCompletion::Timeout : NativeCompletion::Complete;
            }
        };
    }

    std::optional<Luau::SourceCode> Resolver::readSource(const Luau::ModuleName &name) {
        const auto found = captured.find(name);

        if (found == captured.end()) {
            throw std::invalid_argument("captured module source is unavailable: " + name);
        }

        active = name;
        std::string text(found->second.text);

        if (text.compare(0, 3, "\xef\xbb\xbf") == 0) {
            text.replace(0, 3, "   ");
        }

        return Luau::SourceCode{std::move(text), Luau::SourceCode::Module};
    }

    std::optional<Luau::ModuleInfo>
    Resolver::resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) {
        if (!host) {
            throw std::runtime_error("native operation is unavailable");
        }

        const std::string name = active;
        const auto found = captured.find(name);

        if (found == captured.end()) {
            throw std::invalid_argument("captured import context is unavailable");
        }

        const auto start = offset(*host, name, expression->location.begin);
        const auto end = offset(*host, name, expression->location.end);

        for (const auto &site : found->second.sites) {
            if (site.argument_start == start && site.argument_end == end) {
                if (!site.static_resolution || site.target.empty()) {
                    return std::nullopt;
                }

                return Luau::ModuleInfo{std::string(site.target)};
            }
        }

        return std::nullopt;
    }

    std::optional<std::string> Resolver::getEnvironmentForModule(const Luau::ModuleName &name) const {
        const auto found = environments.find(name);

        return found == environments.end() ? std::nullopt : std::optional<std::string>{found->second};
    }

    const Luau::Config &Resolver::getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const {
        const auto found = configurations.find(name);

        if (found == configurations.end()) {
            throw std::invalid_argument("captured module settings are unavailable: " + name);
        }

        return found->second;
    }

    AnalysisState::AnalysisState() {
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
        Luau::freeze(frontend.globals.globalTypes);
    }

    NativeAnalysisResult
    analyze(AnalysisState &state, const NativeAnalysisRequest &input, const NativeOperation &operation) {
        NativeAnalysisResult result{state.revision, {}, NativeCompletion::Complete};
        Interruption interruption(operation);
        state.purpose = input.purpose;
        rust::Vec<rust::String> names;

        for (const auto &module : input.modules) {
            if (module.name.empty() || module.mode > 1 || module.retained.size() > 11) {
                throw std::invalid_argument("invalid captured module input");
            }

            const std::string name(module.name);
            const auto previous = state.resolver.captured.find(name);
            state.outcomes.insert_or_assign(name, ModuleOutcome{});

            if (previous == state.resolver.captured.end() || previous->second.signature != module.signature) {
                state.frontend.markDirty(name);
            }

            state.resolver.captured.insert_or_assign(name, module);
            Luau::Config configuration;

            configuration.mode = module.inference ? (module.mode == 0 ? Luau::Mode::Strict : Luau::Mode::Nonstrict)
                                                  : Luau::Mode::NoCheck;

            configuration.typeErrors = module.reporting;
            configuration.parseOptions.allowDeclarationSyntax = module.declaration_syntax;
            configuration.parseOptions.captureComments = true;

            for (const auto &global : module.globals) {
                configuration.globals.emplace_back(global);
            }

            state.resolver.configurations.insert_or_assign(name, std::move(configuration));
            names.push_back(module.name);
            const auto environment = prepare_environment(state, module, operation);
            NativeModuleAnalysis observed{};
            observed.name = module.name;
            observed.logical_revision = module.logical_revision;
            observed.environment_available = state.outcomes.at(name).environment_available;
            observed.environment_diagnostics = environment.diagnostics;
            observed.completion = environment.completion;

            if (environment.completion == NativeCompletion::Cancelled ||
                environment.completion == NativeCompletion::Timeout) {
                result.completion = environment.completion;
            }

            result.modules.push_back(std::move(observed));
        }

        state.frontend.options.retainFullTypeGraphs = false;

        if (interruption.status() != NativeCompletion::Complete) {
            result.completion = interruption.status();

            for (auto &module : result.modules) {
                module.completion = result.completion;
            }

            state.captured = input;

            return result;
        }

        for (const auto &module : input.modules) {
            state.frontend.options.retainFullTypeGraphs =
                state.frontend.options.retainFullTypeGraphs || !module.retained.empty() || input.purpose == 1;
        }

        state.frontend.options.forAutocomplete = false;
        bool force_strict = false;
        bool force_nonstrict = false;

        for (const auto *flag = Luau::FValue<bool>::list; flag; flag = flag->next) {
            if (std::string_view(flag->name) == "DebugLuauForceStrictMode") {
                force_strict = flag->value;
            } else if (std::string_view(flag->name) == "DebugLuauForceNonStrictMode") {
                force_nonstrict = flag->value;
            }
        }

        parse_modules(state, operation, {names.data(), names.size()});
        bool checking = false;

        for (size_t index = 0; index < input.modules.size(); ++index) {
            const auto &module = input.modules[index];
            auto &observed = result.modules[index];
            const std::string name(module.name);
            auto *source = state.frontend.getSourceModule(name);

            if (!source || !source->root) {
                throw std::invalid_argument("captured native source is unavailable");
            }

            source->mode = module.inference ? (module.mode == 0 ? Luau::Mode::Strict : Luau::Mode::Nonstrict)
                                            : Luau::Mode::NoCheck;

            observed.parsed = true;
            state.outcomes.at(name).parsed = true;

            if (module.inference &&
                ((force_strict && module.mode != 0) || (!force_strict && force_nonstrict && module.mode != 1))) {
                observed.completion = NativeCompletion::Unsupported;
            }

            observed.recovered_syntax = !source->parseErrors.empty();

            for (const auto &error : source->parseErrors) {
                auto finding = diagnostic(
                    operation,
                    Luau::TypeError{error.getLocation(), name, Luau::SyntaxError{error.what()}},
                    name
                );

                operation.finding(finding);
                observed.diagnostics.push_back(std::move(finding));
            }

            for (const auto &comment : source->hotcomments) {
                observed.directives.push_back(
                    NativeDirective{location(operation, name, comment.location), rust::String(comment.content)}
                );
            }

            if (module.inference && !observed.environment_available) {
                observed.completion = NativeCompletion::Environment;
            } else if (module.inference && observed.completion == NativeCompletion::Complete &&
                       interruption.status() == NativeCompletion::Complete) {
                state.frontend.queueModuleCheck(name);
                checking = true;
            }
        }

        if (checking && interruption.status() == NativeCompletion::Complete) {
            state.resolver.host = &operation;

            struct Reset {
                Resolver &resolver;

                ~Reset() { resolver.host = nullptr; }
            } reset{state.resolver};

            auto options = state.frontend.options;
            options.cancellationToken = interruption.token;
            const auto remaining = operation.remaining_nanoseconds();

            if (remaining != std::numeric_limits<uint64_t>::max()) {
                options.moduleTimeLimitSec = double(remaining) / 1000000000.0;
            }

            size_t completed = 0;
            operation.progress("Checking", "", completed, input.modules.size());

            options.customModuleCheck = [&](const Luau::SourceModule &source, const Luau::Module &) {
                operation.progress("Checking", source.name, ++completed, input.modules.size());
            };

            try {
                state.frontend.checkQueuedModules(options, {}, [&](size_t, size_t) {
                    return interruption.status() == NativeCompletion::Complete;
                });
            } catch (const Luau::UserCancelError &) {
                result.completion = NativeCompletion::Cancelled;
            } catch (const Luau::TimeLimitError &) {
                result.completion = NativeCompletion::Timeout;
            } catch (const Luau::InternalCompilerError &error) {
                const std::string name = error.moduleName.value_or(std::string(input.roots[0]));

                for (auto &module : result.modules) {
                    if (std::string(module.name) == name) {
                        auto finding = diagnostic(
                            operation,
                            Luau::TypeError{error.location.value_or(Luau::Location{}),
                                name,
                                Luau::InternalError{error.what()}},
                            name
                        );

                        operation.finding(finding);
                        module.diagnostics.push_back(std::move(finding));
                        module.completion = NativeCompletion::Analysis;
                    }
                }

                result.completion = NativeCompletion::Analysis;
            }
        }

        for (size_t index = 0; index < input.modules.size(); ++index) {
            const auto &module = input.modules[index];
            auto &observed = result.modules[index];
            const std::string name(module.name);

            if (interruption.status() != NativeCompletion::Complete) {
                observed.completion = interruption.status();
            }

            if (module.inference && observed.completion == NativeCompletion::Complete) {
                const auto checked = state.frontend.moduleResolver.getModule(name);
                observed.inferred = checked && !checked->cancelled && !checked->timeout;

                if (!observed.inferred) {
                    observed.completion = NativeCompletion::Analysis;
                }

                const auto outcome = state.frontend.getCheckResult(name, false, false);
                bool type_errors_only = true;

                if (outcome) {
                    if (!outcome->timeoutHits.empty()) {
                        observed.completion = NativeCompletion::Timeout;
                        observed.inferred = false;
                    }

                    for (const auto &error : outcome->errors) {
                        auto finding = diagnostic(operation, error, name);
                        const auto category = finding.kind;
                        type_errors_only = type_errors_only && category == NativeKind::Type;

                        if (category == NativeKind::Syntax) {
                            continue;
                        }

                        const std::string message = Luau::toString(error);

                        if (message.find("memory") != std::string::npos &&
                            message.find("type function") != std::string::npos) {
                            observed.completion = NativeCompletion::Resource;
                        } else if (category == NativeKind::Analysis &&
                                   observed.completion == NativeCompletion::Complete) {
                            observed.completion = NativeCompletion::Analysis;
                        }

                        if (category != NativeKind::Type || module.reporting) {
                            operation.finding(finding);
                            observed.diagnostics.push_back(std::move(finding));
                        }
                    }
                }

                observed.semantic_complete =
                    observed.inferred && outcome && type_errors_only && !observed.recovered_syntax;
            }

            if (observed.completion != NativeCompletion::Complete) {
                observed.inferred = false;
                observed.semantic_complete = false;
            }

            auto &outcome = state.outcomes.at(name);
            outcome.inferred = observed.inferred;
            outcome.graph_available = observed.semantic_complete;

            if (observed.completion != NativeCompletion::Complete && result.completion == NativeCompletion::Complete) {
                result.completion = observed.completion;
            }
        }

        bool changed = true;

        while (changed) {
            changed = false;

            for (const auto &module : input.modules) {
                const std::string name(module.name);
                auto &outcome = state.outcomes.at(name);

                if (!outcome.inferred && !outcome.graph_available) {
                    continue;
                }

                for (const auto &site : module.sites) {
                    if (outcome.inferred && site.static_resolution && !site.target.empty() &&
                        !state.outcomes.at(std::string(site.target)).inferred) {
                        outcome.inferred = false;
                        outcome.graph_available = false;
                        changed = true;
                        break;
                    }

                    if (outcome.graph_available && site.static_resolution &&
                        (site.target.empty() || !state.outcomes.at(std::string(site.target)).graph_available)) {
                        outcome.graph_available = false;
                        changed = true;
                        break;
                    }
                }
            }
        }

        for (auto &module : result.modules) {
            const std::string name(module.name);
            const auto &outcome = state.outcomes.at(name);

            if (module.inferred && !outcome.inferred) {
                module.inferred = false;

                if (module.completion == NativeCompletion::Complete) {
                    module.completion = NativeCompletion::Analysis;
                }

                if (result.completion == NativeCompletion::Complete) {
                    result.completion = NativeCompletion::Analysis;
                }
            }

            module.semantic_complete = outcome.graph_available;
            auto &prerequisites = state.outcomes.at(name).prerequisites;
            prerequisites = module.environment_diagnostics;

            for (const auto &diagnostic : module.diagnostics) {
                if (diagnostic.kind != NativeKind::Type) {
                    prerequisites.push_back(diagnostic);
                }
            }
        }

        state.captured = input;
        result.frontend_revision = state.revision;

        return result;
    }

    void validate(
        const AnalysisState &state, uint64_t frontend_revision, const std::string &name, uint64_t logical_revision,
        bool inference, uint8_t purpose
    ) {
        if (frontend_revision != state.revision || purpose != state.purpose) {
            throw std::invalid_argument("stale native analysis reference");
        }

        const auto found = state.resolver.captured.find(name);

        if (found == state.resolver.captured.end() || found->second.logical_revision != logical_revision) {
            throw std::invalid_argument("native logical analysis identity is unavailable");
        }

        const auto outcome = state.outcomes.find(name);

        if (outcome == state.outcomes.end() || !outcome->second.parsed) {
            throw std::invalid_argument("requested native syntax has not been established");
        }

        if (inference && !outcome->second.inferred) {
            throw std::invalid_argument("requested native inference is unavailable");
        }

        if (inference) {
            const auto checked = state.frontend.moduleResolver.getModule(name);

            if (!checked || !checked->hasModuleScope() || checked->cancelled || checked->timeout ||
                state.frontend.isDirty(name)) {
                throw std::invalid_argument("requested native retained state is unavailable");
            }
        }
    }
}
