#include "analysis.hpp"
#include "linting.hpp"
#include "parsing.hpp"

#include "Luau/Ast.h"
#include "Luau/Linter.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/TypeArena.h"

#include <map>
#include <set>
#include <stdexcept>
#include <string>
#include <utility>

namespace instar::native {
    namespace {
        struct Shadows final : Luau::AstVisitor {
            std::map<Luau::Position, std::pair<Luau::Location, Luau::Location>> declarations;

            void local(const Luau::AstLocal *binding) {
                if (binding && binding->shadow) {
                    declarations.emplace(
                        binding->location.begin, std::make_pair(binding->location, binding->shadow->location)
                    );
                }
            }

            bool visit(Luau::AstStatLocal *node) override {
                for (const auto *binding : node->vars) {
                    local(binding);
                }
                return true;
            }

            bool visit(Luau::AstStatLocalFunction *node) override {
                local(node->name);
                return true;
            }

            bool visit(Luau::AstStatFor *node) override {
                local(node->var);
                return true;
            }

            bool visit(Luau::AstStatForIn *node) override {
                for (const auto *binding : node->vars) {
                    local(binding);
                }
                return true;
            }

            bool visit(Luau::AstExprFunction *node) override {
                for (const auto *binding : node->args) {
                    local(binding);
                }
                return true;
            }
        };

        NativeWarning detach(
            const NativeOperation &operation, const std::string &name, const Luau::LintWarning &warning,
            const Shadows &shadows
        ) {
            NativeWarning result{location(operation, name, warning.location),
                static_cast<int32_t>(warning.code),
                rust::String(Luau::LintWarning::getName(warning.code)),
                rust::String(warning.text), {}};

            if (warning.code == Luau::LintWarning::Code_LocalShadow ||
                warning.code == Luau::LintWarning::Code_DuplicateLocal) {
                const auto found = shadows.declarations.find(warning.location.begin);
                if (found != shadows.declarations.end() && found->second.first == warning.location) {
                    result.related.push_back(NativeRelated{
                        location(operation, name, found->second.second), rust::String("previous declaration")
                    });
                }
            }

            return result;
        }
    }

    NativeLintResult lint(AnalysisState &state, const NativeLintInput &input, const NativeOperation &operation) {
        NativeLintResult result{{}, {}, {}, {}, NativeCompletion::Complete};
        const std::string name(input.name);
        const auto *source = state.frontend.getSourceModule(name);

        if (!source || !source->root || input.mode > 3) {
            throw std::invalid_argument("native lint source or mode is unavailable");
        }

        std::set<std::string> pending{name};
        std::set<std::string> visited;

        while (!pending.empty()) {
            const auto current = *pending.begin();
            pending.erase(pending.begin());

            if (!visited.insert(current).second) {
                continue;
            }

            for (const auto &diagnostic : state.outcomes.at(current).prerequisites) {
                result.diagnostics.push_back(diagnostic);
            }

            for (const auto &site : state.resolver.captured.at(current).sites) {
                if (!site.target.empty()) {
                    pending.insert(std::string(site.target));
                }
            }
        }

        const auto &outcome = state.outcomes.at(name);
        auto checked = outcome.inferred ? state.frontend.moduleResolver.getModule(name) : nullptr;

        if (checked &&
            (!checked->hasModuleScope() || checked->cancelled || checked->timeout || state.frontend.isDirty(name))) {
            checked = nullptr;
        }

        const bool environment_available = outcome.environment_available;
        Luau::ScopePtr environment = state.frontend.globals.globalScope;

        if (checked && checked->hasModuleScope()) {
            environment = checked->getModuleScope()->parent;
        } else if (const auto found = state.resolver.environments.find(name);
            found != state.resolver.environments.end()) {
            environment = state.frontend.getEnvironmentScope(found->second);
        }

        const auto &globals = state.resolver.configurations.at(name).globals;

        if (!checked && !globals.empty()) {
            environment = std::make_shared<Luau::Scope>(environment);

            for (const auto &global : globals) {
                const auto identifier = source->names->get(global.c_str());

                if (identifier.value) {
                    environment->bindings[identifier].typeId = state.frontend.builtinTypes->anyType;
                }
            }
        }

        Luau::LintOptions options;
        options.warningMask = 0;
        std::set<std::string> selected;

        for (const auto &warning : input.warnings) {
            const std::string name(warning);
            const auto code = Luau::LintWarning::parseName(name.c_str());

            if (code == Luau::LintWarning::Code_Unknown || !selected.insert(name).second) {
                throw std::invalid_argument("unknown or duplicate native warning selection");
            }

            const bool semantics =
                code == Luau::LintWarning::Code_TableOperations || code == Luau::LintWarning::Code_DeprecatedApi;

            const bool globals =
                code == Luau::LintWarning::Code_UnknownGlobal || code == Luau::LintWarning::Code_DeprecatedGlobal ||
                code == Luau::LintWarning::Code_BuiltinGlobalWrite || code == Luau::LintWarning::Code_LocalShadow;

            const bool available = (!semantics || checked) && (!globals || environment_available);

            const bool complete = source->parseErrors.empty() && (!semantics || outcome.graph_available);

            result.availability.push_back(
                NativeWarningAvailability{warning, static_cast<uint8_t>(available ? complete ? 0 : 1 : 2)}
            );

            if (available && !complete) {
                result.completion = NativeCompletion::Analysis;
            }

            if (!available) {
                result.completion = NativeCompletion::Analysis;
                continue;
            }

            options.enableWarning(code);
        }

        if (input.mode != 0) {
            options.disableWarning(Luau::LintWarning::Code_UnknownGlobal);
        }

        if (input.mode == 2) {
            options.disableWarning(Luau::LintWarning::Code_ImplicitReturn);
        }

        const auto suppression_options = options;
        options.warningMask &= ~Luau::LintWarning::parseMask(source->hotcomments);
        const bool identical_masks = options.warningMask == suppression_options.warningMask;

        if (operation.requested()) {
            result.completion = NativeCompletion::Cancelled;

            return result;
        }

        if (operation.remaining_nanoseconds() == 0) {
            result.completion = NativeCompletion::Timeout;

            return result;
        }

        Luau::Module syntax(std::make_shared<Luau::TypeArena>());
        syntax.checkedInNewSolver = true;
        Shadows shadows;
        source->root->visit(&shadows);

        const auto warnings = Luau::lint(
            source->root,
            *source->names,
            environment,
            checked ? checked.get() : &syntax,
            source->hotcomments,
            options
        );

        for (const auto &warning : warnings) {
            result.warnings.push_back(detach(operation, name, warning, shadows));

            if (identical_masks) {
                result.suppression_warnings.push_back(detach(operation, name, warning, shadows));
            }
        }

        if (operation.requested()) {
            result.completion = NativeCompletion::Cancelled;

            return result;
        }

        if (operation.remaining_nanoseconds() == 0) {
            result.completion = NativeCompletion::Timeout;

            return result;
        }

        if (!identical_masks) {
            const auto suppression_warnings = Luau::lint(
                source->root,
                *source->names,
                environment,
                checked ? checked.get() : &syntax,
                source->hotcomments,
                suppression_options
            );

            for (const auto &warning : suppression_warnings) {
                result.suppression_warnings.push_back(detach(operation, name, warning, shadows));
            }
        }

        if (operation.requested()) {
            result.completion = NativeCompletion::Cancelled;
        } else if (operation.remaining_nanoseconds() == 0) {
            result.completion = NativeCompletion::Timeout;
        }

        return result;
    }
}
