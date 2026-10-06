#include "diagnostics.hpp"
#include "instar-bridge/src/boundary.rs.h"
#include "state.hpp"

#include "Luau/Ast.h"
#include "Luau/Linter.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "Luau/TypeArena.h"
#include <chrono>
#include <set>
#include <stdexcept>
#include <utility>

namespace instar {
    using frontend::diagnostic;
    using frontend::location;

    namespace {
        struct Bindings final : Luau::AstVisitor {
            std::map<Luau::AstLocal *, NativeFactKind> bindings;

            bool visit(Luau::AstStatLocal *node) override {
                for (Luau::AstLocal *local : node->vars) {
                    if (!local->annotation) {
                        bindings.emplace(local, NativeFactKind::ImplicitAnyLocal);
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *node) override {
                for (Luau::AstLocal *local : node->args) {
                    if (!local->annotation) {
                        bindings.emplace(local, NativeFactKind::ImplicitAnyParameter);
                    }
                }

                return true;
            }
        };

        void facts(const Host &host, const std::string &name, const Luau::Module &module, NativeLintResult &result) {
            Bindings visitor;
            module.root->visit(&visitor);
            std::map<std::pair<size_t, size_t>, NativeFact> ordered;

            for (const auto &[range, scope] : module.scopes) {
                for (const auto &[symbol, binding] : scope->bindings) {
                    const auto found = visitor.bindings.find(symbol.local);

                    if (found == visitor.bindings.end() || !Luau::get<Luau::AnyType>(Luau::follow(binding.typeId))) {
                        continue;
                    }

                    const auto kind = found->second;
                    NativeLocation source = location(host, name, symbol.local->location);
                    const std::pair<size_t, size_t> key{source.start, source.end};

                    ordered.emplace(
                        key,
                        NativeFact{std::move(source),
                            kind,
                            rust::String(
                                kind == NativeFactKind::ImplicitAnyLocal ? "Local binding has an implicit any type"
                                                                         : "Parameter has an implicit any type"
                            )}
                    );
                }
            }

            for (auto &[range, fact] : ordered) {
                result.facts.push_back(std::move(fact));
            }
        }
    } // namespace

    NativeLintResult NativeFrontend::lint(
        const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation,
        rust::Slice<const rust::String> semantic_modules
    ) {
        const auto started = std::chrono::steady_clock::now();

        const auto status = [&] {
            if (cancellation.requested()) {
                return NativeCompletion::Cancelled;
            }

            const double elapsed = std::chrono::duration<double>(std::chrono::steady_clock::now() - started).count();

            return elapsed >= timeout_seconds ? NativeCompletion::Timeout : NativeCompletion::Complete;
        };

        std::set<std::string> semantics;

        for (const rust::String &name : semantic_modules) {
            semantics.emplace(name);
        }

        bool typecheck = !semantics.empty();

        for (const rust::String &name : names) {
            const auto &enabled = state->resolver.getConfig(std::string(name), {}).enabledLint;

            typecheck = typecheck || enabled.isEnabled(Luau::LintWarning::Code_FormatString) ||
                        enabled.isEnabled(Luau::LintWarning::Code_DeprecatedApi) ||
                        enabled.isEnabled(Luau::LintWarning::Code_TableOperations);
        }

        NativeCheck checked = analyze(host, entries, timeout_seconds, names, cancellation, typecheck);
        NativeLintResult result{{}, {}, {}, checked.completion};

        for (NativeDiagnostic &error : checked.diagnostics) {
            if (checked.completion == NativeCompletion::Environment || error.kind != NativeKind::Type) {
                result.diagnostics.push_back(std::move(error));
            }
        }

        if (entries.empty() || status() != NativeCompletion::Complete) {
            result.completion = entries.empty() ? checked.completion : status();

            return result;
        }

        prepare(host, names);

        for (const rust::String &opaque : names) {
            if (status() != NativeCompletion::Complete) {
                result.completion = status();

                return result;
            }

            const std::string name(opaque);
            auto &frontend = state->frontend;
            const auto *source = frontend.getSourceModule(name);

            if (!source || !source->root) {
                throw std::runtime_error("native lint source is unavailable: " + name);
            }

            if (checked.completion == NativeCompletion::Environment) {
                for (const Luau::ParseError &error : source->parseErrors) {
                    const Luau::TypeError syntaxError{error.getLocation(), name, Luau::SyntaxError{error.what()}};
                    result.diagnostics.push_back(diagnostic(host, syntaxError, name));
                }
            }

            const auto module = frontend.moduleResolver.getModule(name);

            const bool typed =
                typecheck && checked.completion == NativeCompletion::Complete && module && !module->cancelled;

            const Luau::Config &configuration = state->resolver.getConfig(name, {});
            const Luau::Mode mode = source->mode.value_or(configuration.mode);
            Luau::LintOptions options = configuration.enabledLint;
            options.warningMask &= ~Luau::LintWarning::parseMask(source->hotcomments);

            if (mode != Luau::Mode::NoCheck) {
                options.disableWarning(Luau::LintWarning::Code_UnknownGlobal);
            }

            if (mode == Luau::Mode::Strict) {
                options.disableWarning(Luau::LintWarning::Code_ImplicitReturn);
            }

            Luau::ScopePtr environment = frontend.globals.globalScope;

            if (typed && module->hasModuleScope()) {
                environment = module->getModuleScope()->parent;
            }

            if (!typed) {
                const auto found = state->resolver.environments.find(name);

                if (found != state->resolver.environments.end()) {
                    environment = frontend.getEnvironmentScope(found->second);
                }
            }

            if (!typed && !configuration.globals.empty()) {
                environment = std::make_shared<Luau::Scope>(environment);

                for (const std::string &global : configuration.globals) {
                    const Luau::AstName symbol = source->names->get(global.c_str());

                    if (symbol.value) {
                        environment->bindings[symbol].typeId = frontend.builtinTypes->anyType;
                    }
                }
            }

            Luau::Module syntax(std::make_shared<Luau::TypeArena>());
            syntax.checkedInNewSolver = true;

            const auto warnings = Luau::lint(
                source->root,
                *source->names,
                environment,
                typed ? module.get() : &syntax,
                source->hotcomments,
                options
            );

            for (const Luau::LintWarning &warning : warnings) {
                result.warnings.push_back(
                    NativeWarning{location(host, name, warning.location),
                        static_cast<int32_t>(warning.code),
                        rust::String(Luau::LintWarning::getName(warning.code)),
                        rust::String(warning.text),
                        configuration.lintErrors || configuration.fatalLint.isEnabled(warning.code)}
                );
            }

            const bool semantic = semantics.count(name) != 0;

            if (semantic && mode == Luau::Mode::NoCheck) {
                result.diagnostics.push_back(
                    NativeDiagnostic{location(host, name, source->root->location),
                        NativeKind::Analysis,
                        0,
                        rust::String("Inferred-any lint facts are unavailable in nocheck mode"),
                        {}}
                );

                if (result.completion == NativeCompletion::Complete) {
                    result.completion = NativeCompletion::Analysis;
                }
            } else if (semantic && typed && source->parseErrors.empty()) {
                facts(host, name, *module, result);
            }
        }

        if (status() != NativeCompletion::Complete) {
            result.completion = status();
        }

        return result;
    }

} // namespace instar
