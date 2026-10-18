#include "environment.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Error.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "Luau/TypeArena.h"
#include "capabilities.hpp"
#include "diagnostic.hpp"
#include "enumerations.hpp"
#include "flags.hpp"
#include "instar-luau/src/bridge.rs.h"

#include <memory>
#include <optional>
#include <utility>

namespace instar::luau {
    namespace {
        void rebase(std::optional<std::string> &symbol, const std::string &prefix, const std::string &replacement) {
            if (symbol && symbol->compare(0, prefix.size(), prefix) == 0) {
                symbol = replacement + symbol->substr(prefix.size());
            }
        }

        void rebase_type(Luau::TypeId type, const std::string &prefix, const std::string &replacement) {
            type = Luau::follow(type);
            rebase(Luau::asMutable(type)->documentationSymbol, prefix, replacement);

            if (auto *table = Luau::getMutable<Luau::TableType>(type)) {
                for (auto &[name, property] : table->props) {
                    rebase(property.documentationSymbol, prefix, replacement);
                }
            } else if (auto *external = Luau::getMutable<Luau::ExternType>(type)) {
                for (auto &[name, property] : external->props) {
                    rebase(property.documentationSymbol, prefix, replacement);
                }
            }
        }

        void assign_documentation(
            const Luau::ScopePtr &scope, const std::string &identity, const std::string &namespace_name
        ) {
            const std::string prefix = identity + "/";
            const std::string replacement = namespace_name + "/";

            for (auto &[name, binding] : scope->bindings) {
                rebase(binding.documentationSymbol, prefix, replacement);
                rebase_type(binding.typeId, prefix, replacement);
            }

            for (auto &[name, type] : scope->exportedTypeBindings) {
                rebase_type(type.type, prefix, replacement);
            }
        }
    }

    DeclarationFailure::DeclarationFailure(const rust::Vec<Diagnostic> &diagnostics) noexcept
        : diagnostics(diagnostics) {}

    rust::String declarations(rust::Slice<const Flag> settings, rust::Slice<const Flag> scoped) {
        const Flags flags(settings);
        flags.require(scoped);

        return Luau::getBuiltinDefinitionSource() + "\ndeclare string: any\ndeclare next: any\ndeclare pairs: any"
                                                    "\ndeclare getmetatable: any\ndeclare setmetatable: any\n";
    }

    rust::String builtins(const Context &host, rust::Slice<const Flag> scoped) {
        const auto settings = host.flags();

        return declarations(rust::Slice<const Flag>{settings.data(), settings.size()}, scoped);
    }

    rust::Vec<Diagnostic> prepare(Luau::Frontend &frontend, const std::string &name, const Environment &environment) {
        const auto scope = std::make_shared<Luau::Scope>(frontend.globals.globalScope);

        struct Restore {
            Luau::Frontend &frontend;
            Luau::ScopePtr scope;
            decltype(Luau::Frontend::prepareModuleScope) callback;

            ~Restore() {
                frontend.prepareModuleScope = std::move(callback);
                frontend.globals.globalScope = scope;
                Luau::freeze(frontend.globals.globalTypes);
            }
        } restore{frontend, frontend.globals.globalScope, std::exchange(frontend.prepareModuleScope, {})};

        Luau::unfreeze(frontend.globals.globalTypes);
        frontend.globals.globalScope = scope;

        if (environment.has_enumerations) {
            construct_enumerations(
                frontend.globals,
                {environment.enumerations.data(), environment.enumerations.size()}
            );
        }

        for (const Definition &definition : environment.definitions) {
            std::string source(definition.source);

            if (source.compare(0, 3, "\xef\xbb\xbf") == 0) {
                source.replace(0, 3, "   ");
            }

            const std::string identity(definition.name);
            const auto loaded = frontend.loadDefinitionFile(frontend.globals, scope, source, identity, false);

            if (!loaded.success) {
                rust::Vec<Diagnostic> diagnostics;

                for (const auto &error : loaded.parseResult.errors) {
                    diagnostics.push_back(diagnose(
                        Luau::TypeError{error.getLocation(), identity, Luau::SyntaxError{error.what()}},
                        *frontend.fileResolver
                    ));
                }

                if (loaded.module) {
                    attribute(*loaded.module);

                    for (const auto &error : loaded.module->errors) {
                        diagnostics.push_back(diagnose(error, *frontend.fileResolver));
                    }
                }

                return diagnostics;
            }

            assign_documentation(scope, identity, std::string(definition.namespace_name));
        }

        if (environment.has_capabilities) {
            apply(frontend.globals, {environment.classes.data(), environment.classes.size()});
        }

        frontend.addEnvironment(name);

        return {};
    }
}
