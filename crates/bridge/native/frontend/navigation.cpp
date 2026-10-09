#include "diagnostics.hpp"
#include "navigation.hpp"
#include "state.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "instar-bridge/src/boundary.rs.h"

#include <algorithm>
#include <map>
#include <set>
#include <stdexcept>
#include <utility>
#include <vector>

namespace instar::frontend {
    namespace {
        struct Declarations final : Luau::AstVisitor {
            const Host &host;
            const std::string &module;
            std::map<std::string, NativeLocation> &locations;

            Declarations(const Host &host, const std::string &module, std::map<std::string, NativeLocation> &locations)
                : host(host), module(module), locations(locations) {}

            bool visit(Luau::AstStatDeclareGlobal *node) override {
                locations.insert_or_assign(
                    std::string("global/") + node->name.value,
                    location(host, module, node->nameLocation)
                );

                return false;
            }

            bool visit(Luau::AstStatDeclareFunction *node) override {
                locations.insert_or_assign(
                    std::string("global/") + node->name.value,
                    location(host, module, node->nameLocation)
                );

                return false;
            }

            bool visit(Luau::AstStatDeclareExternType *node) override {
                locations.insert_or_assign(
                    std::string("type/") + node->name.value,
                    location(host, module, node->location)
                );

                return false;
            }

            bool visit(Luau::AstStatTypeAlias *node) override {
                locations.insert_or_assign(
                    std::string("type/") + node->name.value,
                    location(host, module, node->nameLocation)
                );

                return false;
            }

            bool visit(Luau::AstStatTypeFunction *node) override {
                locations.insert_or_assign(
                    std::string("type/") + node->name.value,
                    location(host, module, node->nameLocation)
                );

                return false;
            }
        };

        bool source_scope(const Luau::Module &module, const Luau::Scope *scope) {
            return std::any_of(module.scopes.begin(), module.scopes.end(), [&](const auto &entry) {
                return entry.second.get() == scope;
            });
        }

        void
        append(const Host &host, const std::string &module, Luau::Location range, rust::Vec<NativeLocation> &result) {
            if (!module.empty() && host.read_document(module).found) {
                result.push_back(location(host, module, range));
            }
        }

        void declared(
            const State &state, const std::string &module, const std::string &key, rust::Vec<NativeLocation> &result
        ) {
            const auto environment = state.resolver.environments.find(module);

            if (environment == state.resolver.environments.end()) {
                return;
            }

            const auto declarations = state.locations.find(environment->second);

            if (declarations == state.locations.end()) {
                return;
            }

            const auto found = declarations->second.find(key);

            if (found != declarations->second.end()) {
                result.push_back(found->second);
            }
        }

        void types(const Host &host, Luau::TypeId initial, rust::Vec<NativeLocation> &result) {
            std::set<Luau::TypeId> visited;
            std::vector<Luau::TypeId> pending{initial};

            while (!pending.empty()) {
                const auto type = Luau::follow(pending.back());
                pending.pop_back();

                if (!visited.insert(type).second) {
                    continue;
                }

                if (const auto *function = Luau::get<Luau::FunctionType>(type)) {
                    if (function->definition && function->definition->definitionModuleName) {
                        append(
                            host,
                            *function->definition->definitionModuleName,
                            function->definition->definitionLocation,
                            result
                        );
                    }
                } else if (const auto *table = Luau::get<Luau::TableType>(type)) {
                    append(host, table->definitionModuleName, table->definitionLocation, result);
                } else if (const auto *external = Luau::get<Luau::ExternType>(type)) {
                    if (external->definitionLocation) {
                        append(host, external->definitionModuleName, *external->definitionLocation, result);
                    }
                } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                    pending.insert(pending.end(), intersection->parts.begin(), intersection->parts.end());
                } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                    pending.insert(pending.end(), union_type->options.begin(), union_type->options.end());
                } else if (const auto *metatable = Luau::get<Luau::MetatableType>(type)) {
                    pending.push_back(metatable->table);
                }
            }
        }

        void alias(
            State &state, const Host &host, const Luau::Module &module, const std::string &name,
            const Luau::AstTypeReference &reference, Luau::ScopePtr scope, rust::Vec<NativeLocation> &result
        ) {
            if (reference.prefix) {
                for (; scope; scope = scope->parent) {
                    const auto imported = scope->importedModules.find(reference.prefix->value);

                    if (imported == scope->importedModules.end()) {
                        continue;
                    }

                    const auto target = state.frontend.moduleResolver.getModule(imported->second);

                    if (target && target->hasModuleScope()) {
                        const auto target_scope = target->getModuleScope();
                        const auto found = target_scope->typeAliasNameLocations.find(reference.name.value);

                        if (found != target_scope->typeAliasNameLocations.end()) {
                            append(host, imported->second, found->second, result);
                        }
                    }

                    return;
                }

                return;
            }

            for (; scope; scope = scope->parent) {
                const auto found = scope->typeAliasNameLocations.find(reference.name.value);

                if (found != scope->typeAliasNameLocations.end()) {
                    if (source_scope(module, scope.get())) {
                        append(host, name, found->second, result);
                    } else {
                        declared(state, name, std::string("type/") + reference.name.value, result);
                    }

                    return;
                }
            }

            declared(state, name, std::string("type/") + reference.name.value, result);
        }
    }

    void register_declarations(
        State &state, const Host &host, Luau::AstStatBlock &root, const std::string &module,
        const std::string &environment
    ) {
        Declarations visitor(host, module, state.locations[environment]);
        root.visit(&visitor);
    }

    rust::Vec<NativeLocation>
    definition(State &state, const Host &host, rust::Str module, uint32_t line, uint32_t column, bool type_definition) {
        const std::string name(module.data(), module.size());
        const auto *source = state.frontend.getSourceModule(name);
        const auto checked = state.frontend.moduleResolver.getModule(name);

        if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
            throw std::runtime_error("semantic module is unavailable");
        }

        const Luau::Position position(line, column);
        rust::Vec<NativeLocation> result;

        if (!type_definition) {
            const auto byte = offset(host, name, position);
            const auto input = host.read_module(name);

            for (const auto &site : input.sites) {
                if (!site.target.empty() && site.argument_start <= byte && byte < site.argument_end) {
                    result.push_back(NativeLocation{site.target, host.revision(site.target), 0, 0});

                    return result;
                }
            }
        }

        const auto scope = Luau::findScopeAtPosition(*checked, position);
        const auto ancestry = Luau::findAstAncestryOfPosition(*source, position, true);

        for (auto node = ancestry.rbegin(); node != ancestry.rend(); ++node) {
            if (const auto *reference = (*node)->as<Luau::AstTypeReference>();
                reference && reference->nameLocation.contains(position)) {
                alias(state, host, *checked, name, *reference, scope, result);

                return result;
            }
        }

        auto expression = Luau::findExprOrLocalAtPosition(*source, position);

        if (!type_definition) {
            if (const auto *local = expression.getLocal()) {
                append(host, name, local->location, result);

                return result;
            }

            if (auto *value = expression.getExpr()) {
                if (const auto *local = value->as<Luau::AstExprLocal>()) {
                    append(host, name, local->local->location, result);

                    return result;
                }

                if (const auto *global = value->as<Luau::AstExprGlobal>(); global && scope) {
                    const auto binding = scope->lookupEx(Luau::Symbol(global->name));

                    if (binding && source_scope(*checked, binding->second)) {
                        append(host, name, binding->first->location, result);
                    } else {
                        declared(state, name, std::string("global/") + global->name.value, result);
                    }

                    if (!result.empty()) {
                        return result;
                    }
                }

                if (const auto *index = value->as<Luau::AstExprIndexName>()) {
                    if (const auto *type = checked->astTypes.find(index->expr)) {
                        property(host, *type, index->index.value, result);
                    }

                    if (!result.empty()) {
                        return result;
                    }
                }
            }
        }

        if (const auto type = inferred_type(*checked, *source, position)) {
            types(host, *type, result);
        }

        return result;
    }

    std::optional<Luau::TypeId>
    inferred_type(const Luau::Module &module, const Luau::SourceModule &source, const Luau::Position &position) {
        auto expression = Luau::findExprOrLocalAtPosition(source, position);

        if (auto *local = expression.getLocal()) {
            for (const auto &entry : module.scopes) {
                const auto found = entry.second->bindings.find(Luau::Symbol(local));

                if (found != entry.second->bindings.end()) {
                    return found->second.typeId;
                }
            }

            return std::nullopt;
        }

        return Luau::findTypeAtPosition(module, source, position);
    }

    void property(const Host &host, Luau::TypeId initial, const std::string &name, rust::Vec<NativeLocation> &result) {
        std::set<Luau::TypeId> visited;
        std::vector<Luau::TypeId> pending{initial};

        while (!pending.empty()) {
            const auto type = Luau::follow(pending.back());
            pending.pop_back();

            if (!visited.insert(type).second) {
                continue;
            }

            if (const auto *table = Luau::get<Luau::TableType>(type)) {
                const auto found = table->props.find(name);

                if (found != table->props.end()) {
                    const auto range = found->second.location ? found->second.location : found->second.typeLocation;

                    if (range) {
                        append(host, table->definitionModuleName, *range, result);
                    }
                }
            } else if (const auto *external = Luau::get<Luau::ExternType>(type)) {
                const auto found = external->props.find(name);

                if (found != external->props.end()) {
                    const auto range = found->second.location ? found->second.location : found->second.typeLocation;

                    if (range) {
                        append(host, external->definitionModuleName, *range, result);
                    }
                } else if (external->parent) {
                    pending.push_back(*external->parent);
                }
            } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                pending.insert(pending.end(), intersection->parts.begin(), intersection->parts.end());
            } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                pending.insert(pending.end(), union_type->options.begin(), union_type->options.end());
            } else if (const auto *metatable = Luau::get<Luau::MetatableType>(type)) {
                pending.push_back(metatable->table);
            }
        }
    }
}
