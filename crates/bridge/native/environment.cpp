#include "environment.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/ConfigResolver.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Instantiation2.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "Luau/TypeArena.h"
#include "capabilities.hpp"
#include "diagnostic.hpp"
#include "enumerations.hpp"
#include "flags.hpp"
#include "instar-bridge/src/bridge.rs.h"

#include <memory>
#include <optional>
#include <stdexcept>
#include <utility>

namespace instar::luau {
    namespace {
        void bind_vectors(Luau::GlobalTypes &globals) {
            const auto scope = globals.globalScope;
            const auto native = scope->lookupType("vector");
            const auto roblox = scope->lookupType("Vector3");
            const auto library = scope->lookupEx(Luau::AstName("vector"));
            auto *external = roblox ? Luau::getMutable<Luau::ExternType>(Luau::follow(roblox->type)) : nullptr;

            if (!native || !external || !library) {
                throw std::invalid_argument("Roblox vector compatibility requires vector and Vector3 declarations.");
            }

            for (auto &[name, property] : external->props) {
                property.writeTy.reset();
            }

            Luau::DenseHashMap<Luau::TypeId, Luau::TypeId> types;
            Luau::DenseHashMap<Luau::TypePackId, Luau::TypePackId> packs;
            types[Luau::follow(native->type)] = Luau::follow(roblox->type);

            Luau::Replacer replacement{Luau::NotNull{&globals.globalTypes},
                Luau::NotNull{&types},
                Luau::NotNull{&packs}};

            const auto adapted = replacement.substitute(library->first->typeId);

            if (!adapted) {
                throw std::runtime_error("Roblox vector library type substitution exceeded its complexity limit.");
            }

            scope->exportedTypeBindings.insert_or_assign("vector", *roblox);
            auto binding = *library->first;
            binding.typeId = *adapted;
            scope->bindings.insert_or_assign(Luau::AstName("vector"), std::move(binding));
        }

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

        void collect(
            rust::Vec<Builtin> &symbols, const std::string &name, Luau::TypeId type,
            const std::optional<std::string> &documentation, bool is_type, bool is_type_function
        ) {
            symbols.push_back({name, documentation.value_or(""), is_type, false, is_type_function});
            type = Luau::follow(type);

            const auto properties = [&](const auto &members) {
                for (const auto &[member, property] : members) {
                    symbols.push_back(
                        {std::string(name).append(".").append(member),
                            property.documentationSymbol.value_or(""),
                            is_type,
                            true,
                            is_type_function}
                    );
                }
            };

            if (const auto *table = Luau::get<Luau::TableType>(type)) {
                properties(table->props);
            } else if (const auto *external = Luau::get<Luau::ExternType>(type)) {
                properties(external->props);
            }
        }

        void collect(rust::Vec<Builtin> &symbols, const Luau::ScopePtr &scope, bool is_type_function) {
            for (const auto &[name, binding] : scope->bindings) {
                collect(symbols, name.c_str(), binding.typeId, binding.documentationSymbol, false, is_type_function);
            }

            for (const auto &[name, binding] : scope->exportedTypeBindings) {
                const auto type = Luau::follow(binding.type);
                collect(symbols, name, type, type->documentationSymbol, true, is_type_function);
            }
        }
    }

    DeclarationFailure::DeclarationFailure(const rust::Vec<Diagnostic> &diagnostics) noexcept
        : diagnostics(diagnostics) {}

    bool builtins(const BuiltinFlags &configuration, rust::Vec<Builtin> &symbols) {
        try {
            const Flags flags(configuration.settings);
            flags.require(configuration.scoped);

            Luau::NullFileResolver files;
            Luau::NullConfigResolver settings;
            Luau::Frontend frontend(Luau::SolverMode::New, &files, &settings);
            Luau::registerBuiltinGlobals(frontend, frontend.globals);
            collect(symbols, frontend.globals.globalScope, false);
            collect(symbols, frontend.globals.globalTypeFunctionScope, true);

            return true;
        } catch (const FlagConflict &) {
            return false;
        }
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

            if (environment.has_capabilities && definition.namespace_name == "@roblox") {
                bind_vectors(frontend.globals);
            }
        }

        if (environment.has_capabilities) {
            apply(frontend.globals, {environment.classes.data(), environment.classes.size()});
        }

        frontend.addEnvironment(name);

        return {};
    }
}
