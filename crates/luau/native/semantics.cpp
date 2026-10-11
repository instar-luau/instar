#include "semantics.hpp"

#include "analysis.hpp"
#include "parsing.hpp"

#include "Luau/AstQuery.h"
#include "Luau/GlobalTypes.h"
#include "Luau/Lexer.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/ToString.h"
#include "Luau/TxnLog.h"
#include "Luau/Type.h"
#include "Luau/TypePack.h"
#include "Luau/TypeUtils.h"
#include "instar-luau/src/boundary/mod.rs.h"

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <map>
#include <optional>
#include <set>
#include <stdexcept>
#include <string>
#include <tuple>
#include <utility>
#include <vector>

namespace instar::native {
    namespace {
        Luau::TypeId
        expression_type(const AnalysisState &state, const Luau::Module &module, const Luau::AstExpr *expression) {
            while (expression) {
                if (const auto *type = module.astTypes.find(expression)) {
                    return *type;
                }

                if (const auto *pack = module.astTypePacks.find(expression)) {
                    if (const auto type = Luau::first(*pack)) {
                        return *type;
                    }

                    auto iterator = Luau::begin(*pack);

                    if (iterator == Luau::end(*pack) && !iterator.tail()) {
                        return state.frontend.builtinTypes->nilType;
                    }
                }

                if (const auto *group = expression->as<Luau::AstExprGroup>()) {
                    expression = group->expr;
                    continue;
                }

                const auto scope = Luau::findScopeAtPosition(module, expression->location.begin);

                if (scope) {
                    if (const auto *local = expression->as<Luau::AstExprLocal>()) {
                        return scope->lookup(Luau::Symbol(local->local)).value_or(nullptr);
                    }

                    if (const auto *global = expression->as<Luau::AstExprGlobal>()) {
                        return scope->lookup(Luau::Symbol(global->name)).value_or(nullptr);
                    }
                }

                return nullptr;
            }

            return nullptr;
        }

        std::optional<Luau::TypeId> inferred_type(
            const AnalysisState &state, const Luau::Module &module, const Luau::SourceModule &source,
            const Luau::Position &position
        );

        void property(
            AnalysisState &state, const NativeOperation &host, Luau::TypeId initial, const std::string &name,
            rust::Vec<NativeLocation> &result
        );

        namespace {

            bool source_scope(const Luau::Module &module, const Luau::Scope *scope) {
                return std::any_of(module.scopes.begin(), module.scopes.end(), [&](const auto &entry) {
                    return entry.second.get() == scope;
                });
            }

            void append(
                const NativeOperation &host, const std::string &module, Luau::Location range,
                rust::Vec<NativeLocation> &result
            ) {
                if (!module.empty() && host.read_document(module).found) {
                    result.push_back(location(host, module, range));
                }
            }

            void declared(
                const std::string &module, const AnalysisState &state, const std::string &key,
                rust::Vec<NativeLocation> &result
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

            void types(const NativeOperation &host, Luau::TypeId initial, rust::Vec<NativeLocation> &result) {
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
                AnalysisState &state, const NativeOperation &host, const Luau::Module &module, const std::string &name,
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
                            declared(name, state, std::string("type/") + reference.name.value, result);
                        }

                        return;
                    }
                }

                declared(name, state, std::string("type/") + reference.name.value, result);
            }
        }

        rust::Vec<NativeLocation> definition(
            AnalysisState &state, const NativeOperation &host, rust::Str module, uint32_t line, uint32_t column,
            bool type_definition
        ) {
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
                            declared(name, state, std::string("global/") + global->name.value, result);
                        }

                        if (!result.empty()) {
                            return result;
                        }
                    }

                    if (const auto *index = value->as<Luau::AstExprIndexName>()) {
                        if (const auto *type = checked->astTypes.find(index->expr)) {
                            property(state, host, *type, index->index.value, result);
                        }

                        if (!result.empty()) {
                            return result;
                        }
                    }
                }
            }

            if (const auto type = inferred_type(state, *checked, *source, position)) {
                types(host, *type, result);
            }

            return result;
        }

        std::optional<Luau::TypeId> inferred_type(
            const AnalysisState &state, const Luau::Module &module, const Luau::SourceModule &source,
            const Luau::Position &position
        ) {
            const auto ancestry = Luau::findAstAncestryOfPosition(source, position, true);

            for (auto node = ancestry.rbegin(); node != ancestry.rend(); ++node) {
                if (const auto *annotation = (*node)->asType()) {
                    if (const auto *type = module.astResolvedTypes.find(annotation)) {
                        return *type;
                    }

                    if (const auto *reference = annotation->as<Luau::AstTypeReference>();
                        reference && reference->nameLocation.contains(position) && reference->parameters.size == 0) {
                        const auto scope = Luau::findScopeAtPosition(module, position);

                        if (scope) {
                            const auto binding = reference->prefix
                                ? scope->lookupImportedType(reference->prefix->value, reference->name.value)
                                : scope->lookupType(reference->name.value);

                            if (binding && binding->typeParams.empty() && binding->typePackParams.empty()) {
                                return binding->type;
                            }
                        }
                    }

                    return std::nullopt;
                }
            }

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

            if (const auto type = expression_type(state, module, expression.getExpr())) {
                return type;
            }

            return std::nullopt;
        }

        std::optional<Luau::DocumentationSymbol> documentation_symbol(
            const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position, Luau::TypeId type
        ) {
            auto expression = Luau::findExprOrLocalAtPosition(source, position);
            const auto *value = expression.getExpr();

            if (expression.getLocal() || (value && value->is<Luau::AstExprLocal>())) {
                return type ? Luau::follow(type)->documentationSymbol : std::nullopt;
            }

            return Luau::getDocumentationSymbolAtPosition(source, module, position);
        }

        std::string property_module(AnalysisState &state, const Luau::TableType &initial) {
            const auto *table = &initial;
            std::set<const Luau::TableType *> visited;

            while (visited.insert(table).second) {
                const auto *source = state.frontend.getSourceModule(table->definitionModuleName);
                const auto checked = state.frontend.moduleResolver.getModule(table->definitionModuleName);

                if (!source || !source->root || !checked) {
                    break;
                }

                const auto scope = Luau::findScopeAtPosition(*checked, table->definitionLocation.begin);

                if (!scope) {
                    break;
                }

                const auto ancestry = Luau::findAstAncestryOfPosition(*source, table->definitionLocation.begin, true);
                std::optional<Luau::TypeFun> binding;

                for (auto node = ancestry.rbegin(); node != ancestry.rend(); ++node) {
                    const auto *reference = (*node)->as<Luau::AstTypeReference>();

                    if (reference && reference->location == table->definitionLocation) {
                        binding = reference->prefix
                                      ? scope->lookupImportedType(reference->prefix->value, reference->name.value)
                                      : scope->lookupType(reference->name.value);
                        break;
                    }
                }

                const auto *original = binding ? Luau::getTableType(binding->type) : nullptr;

                if (!original) {
                    break;
                }

                table = original;
            }

            return table->definitionModuleName;
        }

        void property(
            AnalysisState &state, const NativeOperation &host, Luau::TypeId initial, const std::string &name,
            rust::Vec<NativeLocation> &result
        ) {
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
                            append(host, property_module(state, *table), *range, result);
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
}

namespace instar::native {
    namespace {
        NativeHover
        hover(AnalysisState &state, const NativeOperation &host, rust::Str module, uint32_t line, uint32_t column) {
            const std::string name(module.data(), module.size());
            const auto *source = state.frontend.getSourceModule(name);
            const auto checked = state.frontend.moduleResolver.getModule(name);

            if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
                throw std::runtime_error("semantic module is unavailable");
            }

            const Luau::Position position(line, column);
            const auto type = inferred_type(state, *checked, *source, position);
            auto expression = Luau::findExprOrLocalAtPosition(*source, position);
            auto range = expression.getLocation();

            if (const auto *value = expression.getExpr()) {
                if (const auto *member = value->as<Luau::AstExprIndexName>();
                    member && member->indexLocation.contains(position)) {
                    range = member->indexLocation;
                }
            }

            const auto ancestry = Luau::findAstAncestryOfPosition(*source, position, true);

            for (auto node = ancestry.rbegin(); node != ancestry.rend(); ++node) {
                if (const auto *annotation = (*node)->asType()) {
                    const auto *reference = annotation->as<Luau::AstTypeReference>();
                    range = reference && reference->nameLocation.contains(position)
                        ? reference->nameLocation
                        : annotation->location;
                    break;
                }
            }

            if (!type || !range) {
                return {};
            }

            Luau::ToStringOptions options;
            options.functionTypeArguments = true;
            options.scope = Luau::findScopeAtPosition(*checked, position);

            const auto documentation = documentation_symbol(*checked, *source, position, *type);

            const auto binding = Luau::findBindingAtPosition(*checked, *source, position);
            const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*type));
            const bool deprecated = (binding && binding->deprecated) || (function && function->isDeprecatedFunction);

            return NativeHover{true,
                location(host, name, *range),
                rust::String(Luau::toString(*type, options)),
                rust::String(documentation.value_or("")),
                deprecated,
                Luau::get<Luau::AnyType>(Luau::follow(*type)) != nullptr};
        }
    }
}

namespace instar::native {
    namespace {
        namespace {
            enum class Category : uint8_t { Binding, Type, Property, Receiver, Method };

            struct References final : Luau::AstVisitor {
                AnalysisState &state;
                const NativeOperation &host;
                const std::string &name;
                const Luau::SourceModule &source;
                const Luau::Module &checked;
                rust::Vec<NativeReference> &result;
                std::set<const Luau::AstExpr *> writes;
                std::set<std::pair<size_t, size_t>> read_writes;
                std::set<const Luau::AstLocal *> loops;
                std::map<const Luau::AstLocal *, Luau::Location> receivers;
                std::map<const Luau::AstLocal *, Luau::TypeId> bindings;
                std::set<const Luau::AstLocal *> parameters;

                References(
                    AnalysisState &state, const NativeOperation &host, const std::string &name,
                    const Luau::SourceModule &source, const Luau::Module &checked, rust::Vec<NativeReference> &result
                )
                    : state(state), host(host), name(name), source(source), checked(checked), result(result) {
                    for (const auto &entry : checked.scopes) {
                        for (const auto &[symbol, binding] : entry.second->bindings) {
                            if (symbol.local) {
                                bindings.try_emplace(symbol.local, binding.typeId);
                            }
                        }
                    }
                }

                bool visit(Luau::AstStatLocal *node) override {
                    for (const auto *binding : node->vars) {
                        local(*binding);
                    }

                    return true;
                }

                bool visit(Luau::AstStatLocalFunction *node) override {
                    local(*node->name, true);

                    return true;
                }

                bool visit(Luau::AstStatFor *node) override {
                    loops.insert(node->var);
                    local(*node->var);

                    return true;
                }

                bool visit(Luau::AstStatForIn *node) override {
                    for (const auto *binding : node->vars) {
                        loops.insert(binding);
                        local(*binding);
                    }

                    return true;
                }

                bool visit(Luau::AstExprFunction *node) override {
                    if (node->self) {
                        receivers.insert_or_assign(node->self, node->location);
                    }

                    for (const auto *binding : node->args) {
                        parameters.insert(binding);
                        local(*binding);
                    }

                    return true;
                }

                bool visit(Luau::AstStatTypeAlias *node) override {
                    const auto *type = checked.astResolvedTypes.find(node->type);
                    declared(node->name.value, node->nameLocation, Category::Type, type ? *type : nullptr);

                    return true;
                }

                bool visit(Luau::AstExprLocal *node) override {
                    const auto receiver = receivers.find(node->local);
                    rust::Vec<NativeLocation> targets;

                    targets.push_back(
                        location(host, name, receiver == receivers.end() ? node->local->location : receiver->second)
                    );

                    append(
                        node->local->name.value,
                        node->location,
                        receiver == receivers.end() ? Category::Binding : Category::Receiver,
                        std::move(targets),
                        false,
                        writes.find(node) != writes.end(),
                        false,
                        expression_type(state, checked, node),
                        parameters.find(node->local) != parameters.end()
                    );

                    return true;
                }

                bool visit(Luau::AstExprGlobal *node) override {
                    const auto position = node->location.begin;
                    auto targets = definition(state, host, name, position.line, position.column, false);
                    const auto binding = checked.getModuleScope()->lookupEx(Luau::Symbol(node->name));

                    append(
                        node->name.value,
                        node->location,
                        Category::Binding,
                        std::move(targets),
                        false,
                        writes.find(node) != writes.end(),
                        false,
                        expression_type(state, checked, node),
                        false,
                        binding && binding->first->deprecated
                    );

                    return true;
                }

                bool visit(Luau::AstExprIndexName *node) override {
                    if (const auto *type = checked.astTypes.find(node->expr)) {
                        member(
                            *type,
                            node->index.value,
                            node->indexLocation,
                            false,
                            writes.find(node) != writes.end(),
                            false,
                            expression_type(state, checked, node),
                            node->op == ':'
                        );
                    }

                    return true;
                }

                bool visit(Luau::AstExprIndexExpr *node) override {
                    const auto *key = node->index->as<Luau::AstExprConstantString>();
                    const auto *type = checked.astTypes.find(node->expr);

                    if (key && type) {
                        member(
                            *type,
                            std::string(key->value.data, key->value.size),
                            key->location,
                            false,
                            writes.find(node) != writes.end(),
                            true,
                            expression_type(state, checked, node)
                        );
                    }

                    return true;
                }

                bool visit(Luau::AstExprTable *node) override {
                    const auto *type = checked.astTypes.find(node);

                    if (!type) {
                        return true;
                    }

                    for (const auto &item : node->items) {
                        if (const auto *key = item.key ? item.key->as<Luau::AstExprConstantString>() : nullptr) {
                            member(
                                *type,
                                std::string(key->value.data, key->value.size),
                                key->location,
                                true,
                                true,
                                item.kind == Luau::AstExprTable::Item::Kind::General,
                                expression_type(state, checked, item.value)
                            );
                        }
                    }

                    return true;
                }

                bool visit(Luau::AstType *node) override { return true; }

                bool visit(Luau::AstTypePack *node) override { return true; }

                bool visit(Luau::AstTypeReference *node) override {
                    if (node->prefixLocal && node->prefixLocation) {
                        rust::Vec<NativeLocation> targets;
                        targets.push_back(location(host, name, node->prefixLocal->location));

                        append(
                            node->prefixLocal->name.value,
                            *node->prefixLocation,
                            Category::Binding,
                            std::move(targets),
                            false,
                            false
                        );
                    }

                    const auto position = node->nameLocation.begin;
                    auto targets = definition(state, host, name, position.line, position.column, false);
                    const auto *type = checked.astResolvedTypes.find(node);

                    append(
                        node->name.value,
                        node->nameLocation,
                        Category::Type,
                        std::move(targets),
                        false,
                        false,
                        false,
                        type ? *type : nullptr
                    );

                    return true;
                }

                bool visit(Luau::AstTypeTable *node) override {
                    if (const auto *type = checked.astResolvedTypes.find(node)) {
                        for (const auto &field : node->props) {
                            const auto *value = checked.astResolvedTypes.find(field.type);

                            member(
                                *type,
                                field.name.value,
                                field.location,
                                true,
                                true,
                                false,
                                value ? *value : nullptr
                            );
                        }
                    }

                    return true;
                }

                bool visit(Luau::AstStatAssign *node) override {
                    writes.insert(node->vars.begin(), node->vars.end());

                    return true;
                }

                bool visit(Luau::AstStatCompoundAssign *node) override {
                    writes.insert(node->var);

                    read_writes.emplace(
                        offset(host, name, node->var->location.begin),
                        offset(host, name, node->var->location.end)
                    );

                    return true;
                }

                bool visit(Luau::AstStatFunction *node) override {
                    writes.insert(node->name);

                    return true;
                }

              private:
                void local(const Luau::AstLocal &binding, bool function_declaration = false) {
                    const auto type = bindings.find(&binding);
                    const size_t previous = result.size();

                    declared(
                        binding.name.value,
                        binding.location,
                        Category::Binding,
                        type == bindings.end() ? nullptr : type->second,
                        parameters.find(&binding) != parameters.end()
                    );

                    if (result.size() != previous) {
                        auto &reference = result[result.size() - 1];
                        reference.annotation = binding.annotation != nullptr;

                        reference.binding_kind = reference.parameter     ? 1
                                                 : loops.count(&binding) ? 2
                                                 : function_declaration  ? 3
                                                                         : 0;

                        reference.has_shadow = binding.shadow != nullptr;

                        if (binding.shadow) {
                            reference.shadow = location(host, name, binding.shadow->location);
                        }
                    }
                }

                void declared(
                    const std::string &label, Luau::Location range, Category category, Luau::TypeId type = nullptr,
                    bool parameter = false
                ) {
                    rust::Vec<NativeLocation> targets;
                    targets.push_back(location(host, name, range));
                    append(label, range, category, std::move(targets), true, true, false, type, parameter);
                }

                void member(
                    Luau::TypeId type, const std::string &label, Luau::Location range, bool declaration, bool writing,
                    bool literal, Luau::TypeId value = nullptr, bool method = false
                ) {
                    rust::Vec<NativeLocation> targets;
                    property(state, host, type, label, targets);

                    append(
                        label,
                        range,
                        method ? Category::Method : Category::Property,
                        std::move(targets),
                        declaration,
                        writing,
                        literal,
                        value
                    );
                }

                void append(
                    const std::string &label, Luau::Location range, Category category,
                    rust::Vec<NativeLocation> targets, bool declaration, bool writing, bool literal = false,
                    Luau::TypeId type = nullptr, bool parameter = false, bool deprecated = false
                ) {
                    if (range.begin == range.end) {
                        return;
                    }

                    const auto reference = location(host, name, range);

                    const auto documentation =
                        documentation_symbol(checked, source, range.begin, type)
                            .value_or(type ? Luau::follow(type)->documentationSymbol.value_or("") : "");

                    declaration = declaration || std::any_of(targets.begin(), targets.end(), [&](const auto &target) {
                        return target.module == reference.module && target.revision == reference.revision &&
                               target.start == reference.start && target.end == reference.end;
                    });

                    const auto *function = type ? Luau::get<Luau::FunctionType>(Luau::follow(type)) : nullptr;
                    const auto *overloads = type ? Luau::get<Luau::IntersectionType>(Luau::follow(type)) : nullptr;

                    const bool callable =
                        function ||
                        (overloads && !overloads->parts.empty() &&
                            std::all_of(overloads->parts.begin(), overloads->parts.end(), [](Luau::TypeId part) {
                                return Luau::get<Luau::FunctionType>(Luau::follow(part)) != nullptr;
                            }));

                    const auto scope = Luau::findScopeAtPosition(checked, range.begin);

                    result.push_back(
                        NativeReference{reference,
                            std::move(targets),
                            rust::String(documentation),
                            rust::String(label),
                            static_cast<uint8_t>(category),
                            declaration,
                            writing,
                            literal,
                            callable,
                            parameter,
                            deprecated || (function && function->isDeprecatedFunction),
                            (!declaration && !writing) || read_writes.count({reference.start, reference.end}) != 0,
                            false,
                            static_cast<uint8_t>(
                                parameter                                                        ? 1
                                : category == Category::Type                                     ? 6
                                : category == Category::Property || category == Category::Method ? 5
                                : category == Category::Receiver                                 ? 7
                                : declaration                                                    ? 0
                                                                                                 : 4
                            ),
                            rust::String(type ? Luau::toString(type) : ""),
                            type != nullptr,
                            type && Luau::get<Luau::AnyType>(Luau::follow(type)),
                            location(host, name, scope ? scope->location : source.root->location),
                            {},
                            false}
                    );

                    if (type) {
                        types(host, type, result[result.size() - 1].type_declarations);
                    }
                }
            };
        }

        rust::Vec<NativeReference>
        references(AnalysisState &state, const NativeOperation &host, rust::Slice<const rust::String> modules) {
            rust::Vec<NativeReference> result;

            for (const auto &module : modules) {
                const std::string name(module.data(), module.size());
                const auto *source = state.frontend.getSourceModule(name);
                const auto checked = state.frontend.moduleResolver.getModule(name);

                if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
                    throw std::runtime_error("semantic module is unavailable");
                }

                References visitor(state, host, name, *source, *checked, result);
                source->root->visit(&visitor);
            }

            return result;
        }
    }
}

namespace instar::native {
    namespace {
        namespace {
            const Luau::AstExprCall *call_at(const Luau::SourceModule &source, Luau::Position position) {
                const auto ancestry = Luau::findAncestryAtPositionForAutocomplete(source, position);

                for (auto node = ancestry.rbegin(); node != ancestry.rend(); ++node) {
                    if ((*node)->is<Luau::AstExprFunction>()) {
                        return nullptr;
                    }

                    if (const auto *call = (*node)->as<Luau::AstExprCall>();
                        call && call->argLocation.containsClosed(position)) {
                        return call;
                    }
                }

                return nullptr;
            }

            size_t argument(const NativeDocument &document, const Luau::AstExprCall &call, Luau::Position position) {
                Luau::Allocator allocator;
                Luau::AstNameTable names(allocator);
                Luau::Lexer lexer(document.text.data(), document.text.size(), names);
                size_t result = 0;

                for (auto token = lexer.next(); token.type != Luau::Lexeme::Eof && token.location.end <= position;
                    token = lexer.next()) {
                    if (token.type != ',' || token.location.begin < call.argLocation.begin) {
                        continue;
                    }

                    bool nested = false;

                    for (const auto *value : call.args) {
                        if (value->location.contains(token.location.begin)) {
                            nested = true;
                            break;
                        }
                    }

                    if (!nested) {
                        ++result;
                    }
                }

                return result;
            }

            NativeSignature detach(
                const NativeOperation &host, const std::string &name, const Luau::FunctionType &function,
                size_t receivers, Luau::ToStringOptions &options
            ) {
                auto [head, tail] = Luau::flatten(function.argTypes);
                const auto skip = std::min(receivers, head.size());
                auto display = function;
                auto argument_start = head.begin();
                auto name_start = display.argNames.begin();

                for (size_t index = 0; index < skip; ++index) {
                    ++argument_start;

                    if (name_start != display.argNames.end()) {
                        ++name_start;
                    }
                }

                head.erase(head.begin(), argument_start);
                display.argNames.erase(display.argNames.begin(), name_start);
                Luau::TypePackVar arguments{Luau::TypePack{std::move(head), tail}};
                display.argTypes = &arguments;

                const auto *variadic = tail ? Luau::get<Luau::VariadicTypePack>(Luau::follow(*tail)) : nullptr;
                NativeSignature result{};
                result.variadic = tail.has_value() && (!variadic || !variadic->hidden);
                std::string label = name;

                if (!options.hideNamedFunctionTypeParameters &&
                    (!display.generics.empty() || !display.genericPacks.empty())) {
                    label += "<";
                    bool first = true;

                    const auto generic = [&](const std::string &text) {
                        if (!first) {
                            label += ", ";
                        }

                        first = false;
                        label += text;
                    };

                    for (const auto type : display.generics) {
                        generic(Luau::toString(type, options));
                    }

                    for (const auto pack : display.genericPacks) {
                        generic(Luau::toString(pack, options));
                    }

                    label += ">";
                }

                label += "(";

                const auto parameter = [&](const std::string &text) {
                    if (!result.parameters.empty()) {
                        label += ", ";
                    }

                    const auto start = label.size();
                    label += text;
                    result.parameters.push_back(NativeParameter{start, label.size()});
                };

                const auto &visible = Luau::get<Luau::TypePack>(&arguments)->head;

                for (size_t index = 0; index < visible.size(); ++index) {
                    const auto argument_name = index < options.namedFunctionOverrideArgNames.size()
                                                   ? options.namedFunctionOverrideArgNames[index]
                                               : index < display.argNames.size() && display.argNames[index]
                                                   ? display.argNames[index]->name
                                                   : "_";
                    parameter(argument_name + ": " + Luau::toString(visible[index], options));
                }

                if (result.variadic) {
                    parameter(
                        "...: " + (variadic ? Luau::toString(variadic->ty, options) : Luau::toString(*tail, options))
                    );
                }

                label += "): ";
                const auto return_size = Luau::size(display.retTypes);
                const auto return_tail = !Luau::finite(display.retTypes);
                const bool wrap = Luau::get<Luau::TypePack>(Luau::follow(display.retTypes)) &&
                                  (return_tail ? return_size != 0 : return_size > 1);

                if (wrap) {
                    label += "(";
                }

                label += Luau::toString(display.retTypes, options);

                if (wrap) {
                    label += ")";
                }

                result.label = rust::String(label);

                if (!tail || variadic) {
                    result.minimum_arguments = Luau::getParameterExtents(Luau::TxnLog::empty(), &arguments).first;
                    result.has_minimum_arguments = true;
                }

                const auto definition_module =
                    function.definition.has_value() ? function.definition.value().definitionModuleName : std::nullopt;

                if (definition_module.has_value() && host.read_document(definition_module.value()).found) {
                    for (size_t index = 0; index < result.parameters.size() && index < display.argNames.size();
                        ++index) {
                        const auto &parameter = display.argNames[index];

                        if (parameter.has_value()) {
                            result.parameters[index].declaration =
                                location(host, definition_module.value(), parameter.value().location);

                            result.parameters[index].has_declaration = true;
                        }
                    }
                }

                return result;
            }

            std::vector<Luau::TypeId> functions(Luau::TypeId initial) {
                std::vector<Luau::TypeId> result;
                std::vector<Luau::TypeId> pending{initial};
                std::set<Luau::TypeId> seen;

                while (!pending.empty()) {
                    const auto type = Luau::follow(pending.back());
                    pending.pop_back();

                    if (!seen.insert(type).second) {
                        continue;
                    }

                    if (Luau::get<Luau::FunctionType>(type)) {
                        result.push_back(type);
                    } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                        pending.insert(pending.end(), intersection->parts.rbegin(), intersection->parts.rend());
                    }
                }

                return result;
            }
        }

        NativeSignatures signatures(
            AnalysisState &state, const NativeOperation &host, rust::Str module, uint32_t line, uint32_t column
        ) {
            const std::string name(module.data(), module.size());
            const auto *source = state.frontend.getSourceModule(name);
            const auto checked = state.frontend.moduleResolver.getModule(name);

            if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
                throw std::runtime_error("semantic module is unavailable");
            }

            const Luau::Position position(line, column);
            const auto *call = call_at(*source, position);

            if (!call) {
                return {};
            }

            const auto *original = checked->astOriginalCallTypes.find(call);
            const auto *callee = checked->astTypes.find(call->func);

            if (!original) {
                original = callee;
            }

            if (!original) {
                return {};
            }

            auto candidates = functions(*original);
            const auto *selected = checked->astOverloadResolvedTypes.find(call);
            const auto selection = selected ? Luau::follow(*selected) : nullptr;

            if (selection && Luau::get<Luau::FunctionType>(selection) &&
                std::find(candidates.begin(), candidates.end(), selection) == candidates.end()) {
                candidates.push_back(selection);
            }

            if (candidates.empty()) {
                return {};
            }

            size_t receivers = call->self ? 1 : 0;

            if (callee) {
                const auto type = Luau::follow(*callee);

                if (Luau::get<Luau::MetatableType>(type) || Luau::get<Luau::TableType>(type) ||
                    Luau::get<Luau::ExternType>(type)) {
                    ++receivers;
                }
            }

            Luau::ToStringOptions options;
            options.functionTypeArguments = true;
            options.scope = Luau::findScopeAtPosition(*checked, position);
            const auto identifier = Luau::getIdentifier(call->func);
            auto documentation_position = call->func->location.begin;

            if (const auto *member = call->func->as<Luau::AstExprIndexName>()) {
                documentation_position = member->indexLocation.begin;
            }

            NativeSignatures result{};
            result.found = true;
            result.location = location(host, name, call->location);
            result.argument = argument(host.read_document(name), *call, position);

            result.documentation = rust::String(
                Luau::getDocumentationSymbolAtPosition(*source, *checked, documentation_position).value_or("")
            );

            for (const auto type : candidates) {
                if (type == selection || candidates.size() == 1) {
                    result.active = result.entries.size();
                    result.has_active = true;
                }

                result.entries.push_back(detach(
                    host,
                    identifier.value ? identifier.value : "function",
                    *Luau::get<Luau::FunctionType>(type),
                    receivers,
                    options
                ));
            }

            return result;
        }
    }
}

namespace instar::native {
    namespace {
        namespace {
            enum class Kind : uint8_t { Module, Function, Class };

            using Identity = std::tuple<Kind, std::string, size_t, size_t, std::string>;

            struct Pending {
                size_t caller;
                Luau::TypeId type;
                NativeLocation location;
            };

            struct Graph {
                const NativeOperation &host;
                const Luau::BuiltinTypes &builtin_types;
                NativeHierarchy result;
                std::map<Identity, size_t> identities;
                std::map<std::string, NativeDocument> documents;
                std::map<const Luau::ExternType *, std::optional<size_t>> classes;
                std::set<std::pair<size_t, size_t>> inheritance;
                std::set<const Luau::Scope *> scopes;
                std::vector<Pending> pending;

                Graph(const NativeOperation &host, const Luau::GlobalTypes &global_types)
                    : host(host), builtin_types(*global_types.builtinTypes) {
                    result.inheritance_complete = true;
                    scopes.insert(global_types.globalScope.get());
                }

                size_t module(const std::string &name) {
                    const auto &source = document(name);

                    if (!source.found) {
                        throw std::runtime_error("hierarchy source is unavailable");
                    }

                    const auto index = result.items.size();

                    result.items.push_back(
                        NativeHierarchyItem{rust::String(name),
                            static_cast<uint8_t>(Kind::Module),
                            NativeLocation{rust::String(name), source.revision, 0, source.text.size()},
                            0,
                            0,
                            0,
                            source.text.size()}
                    );

                    return index;
                }

                size_t
                function(const std::string &name, const Luau::AstExprFunction &node, const Luau::Module &checked) {
                    auto range = location(host, name, node.location);
                    const Identity identity{Kind::Function, name, range.start, range.end, ""};

                    if (const auto found = identities.find(identity); found != identities.end()) {
                        return found->second;
                    }

                    auto selection_start = range.start;
                    auto selection_end = range.start;

                    if (const auto *type = checked.astTypes.find(&node)) {
                        const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*type));

                        if (function && function->definition && function->definition->definitionModuleName == name &&
                            function->definition->definitionLocation == node.location) {
                            const auto selection = location(host, name, function->definition->originalNameLocation);
                            selection_start = selection.start;
                            selection_end = selection.end;
                            range.start = std::min(range.start, selection_start);
                            range.end = std::max(range.end, selection_end);
                        }
                    }

                    const auto index = result.items.size();
                    identities.emplace(identity, index);

                    result.items.push_back(
                        NativeHierarchyItem{rust::String(node.debugname.value ? node.debugname.value : "function"),
                            static_cast<uint8_t>(Kind::Function),
                            std::move(range),
                            selection_start,
                            selection_end,
                            offset(host, name, node.body->location.begin),
                            offset(host, name, node.body->location.end)}
                    );

                    auto &item = result.items[index];

                    if (const auto *type = checked.astTypes.find(&node)) {
                        item.signatures.location = item.location;

                        item.signatures.documentation =
                            rust::String(Luau::follow(*type)->documentationSymbol.value_or(""));

                        Luau::ToStringOptions options;
                        options.functionTypeArguments = true;
                        options.scope = Luau::findScopeAtPosition(checked, node.location.begin);

                        for (const auto candidate : functions(*type)) {
                            item.signatures.entries.push_back(detach(
                                host,
                                std::string(item.name),
                                *Luau::get<Luau::FunctionType>(candidate),
                                0,
                                options
                            ));
                        }

                        item.signatures.found = !item.signatures.entries.empty();
                    }

                    return index;
                }

                void scope(Luau::ScopePtr scope) {
                    for (; scope && scopes.insert(scope.get()).second; scope = scope->parent) {
                        std::map<std::string, Luau::TypeId> types;

                        for (const auto &[name, type] : scope->exportedTypeBindings) {
                            types.insert_or_assign(name, type.type);
                        }

                        for (const auto &[name, type] : scope->privateTypeBindings) {
                            types.insert_or_assign(name, type.type);
                        }

                        for (const auto &[name, type] : types) {
                            nominal(type);
                        }
                    }
                }

                void call(size_t caller, Luau::TypeId type, const std::string &module, Luau::Location range) {
                    pending.push_back(Pending{caller, type, location(host, module, range)});
                }

                void select(Luau::TypeId initial) {
                    std::vector<Luau::TypeId> pending{initial};
                    std::set<Luau::TypeId> visited;
                    std::set<size_t> selected;

                    while (!pending.empty()) {
                        const auto type = Luau::follow(pending.back());
                        pending.pop_back();

                        if (!visited.insert(type).second) {
                            continue;
                        }

                        if (const auto index = function_index(type)) {
                            selected.insert(*index);
                        } else if (const auto index = nominal(type)) {
                            selected.insert(*index);
                        } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                            pending.insert(pending.end(), union_type->options.begin(), union_type->options.end());
                        }
                    }

                    for (const auto index : selected) {
                        result.selected.push_back(index);
                    }
                }

                NativeHierarchy finish() {
                    for (auto &call : pending) {
                        if (const auto callee = function_index(call.type)) {
                            result.calls.push_back(NativeCall{call.caller, *callee, std::move(call.location)});
                        }
                    }

                    for (const auto &[child, parent] : inheritance) {
                        result.inheritance.push_back(NativeInheritance{child, parent});
                    }

                    return std::move(result);
                }

              private:
                const NativeDocument &document(const std::string &name) {
                    auto found = documents.find(name);

                    if (found == documents.end()) {
                        found = documents.emplace(name, host.read_document(name)).first;
                    }

                    return found->second;
                }

                std::optional<size_t> function_index(Luau::TypeId initial) const {
                    std::vector<Luau::TypeId> pending{initial};
                    std::set<Luau::TypeId> visited;
                    std::optional<size_t> result;

                    while (!pending.empty()) {
                        const auto type = Luau::follow(pending.back());
                        pending.pop_back();

                        if (!visited.insert(type).second) {
                            continue;
                        }

                        if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                            pending.insert(pending.end(), intersection->parts.begin(), intersection->parts.end());
                            continue;
                        }

                        const auto *function = Luau::get<Luau::FunctionType>(type);

                        if (!function || !function->definition || !function->definition->definitionModuleName) {
                            return std::nullopt;
                        }

                        const auto &module = *function->definition->definitionModuleName;
                        const auto source = documents.find(module);

                        if (source == documents.end() || !source->second.found) {
                            return std::nullopt;
                        }

                        const auto range = location(host, module, function->definition->definitionLocation);

                        const auto found =
                            identities.find(Identity{Kind::Function, module, range.start, range.end, ""});

                        if (found == identities.end() || (result && *result != found->second)) {
                            return std::nullopt;
                        }

                        result = found->second;
                    }

                    return result;
                }

                std::optional<size_t> nominal(Luau::TypeId initial) {
                    std::optional<size_t> first;
                    std::optional<size_t> previous;
                    auto type = Luau::follow(initial);

                    while (const auto *external = Luau::get<Luau::ExternType>(type)) {
                        if ((type == builtin_types.externType || type == builtin_types.objectType ||
                                type == builtin_types.classType) &&
                            !external->definitionLocation && !external->parent) {
                            break;
                        }

                        if (const auto found = classes.find(external); found != classes.end()) {
                            if (previous && found->second && *previous != *found->second) {
                                inheritance.emplace(*previous, *found->second);
                            }

                            return first ? first : found->second;
                        }

                        const auto &module = external->definitionModuleName;

                        if (!external->definitionLocation ||
                            external->definitionLocation->begin == external->definitionLocation->end ||
                            module.empty() || !document(module).found) {
                            result.inheritance_complete = false;
                            classes.emplace(external, std::nullopt);
                            break;
                        }

                        auto range = location(host, module, *external->definitionLocation);
                        const Identity identity{Kind::Class, module, range.start, range.end, external->name};
                        auto found = identities.find(identity);

                        if (found == identities.end()) {
                            const auto index = result.items.size();
                            found = identities.emplace(identity, index).first;
                            const auto selection = range.start;
                            const auto selection_end = selection + external->name.size();
                            const auto &source = document(module);

                            if (selection_end > range.end || selection_end > source.text.size() ||
                                std::string(source.text.data() + selection, external->name.size()) != external->name) {
                                throw std::runtime_error("nominal declaration name disagrees with captured source");
                            }

                            result.items.push_back(
                                NativeHierarchyItem{rust::String(external->name),
                                    static_cast<uint8_t>(Kind::Class),
                                    std::move(range),
                                    selection,
                                    selection_end,
                                    selection,
                                    selection}
                            );
                        }

                        const auto index = found->second;
                        classes.emplace(external, index);

                        if (!first) {
                            first = index;
                        }

                        if (previous && *previous != index) {
                            inheritance.emplace(*previous, index);
                        }

                        previous = index;

                        if (!external->parent) {
                            break;
                        }

                        type = Luau::follow(*external->parent);
                    }

                    return first;
                }
            };

            struct Collector final : Luau::AstVisitor {
                Graph &graph;
                const std::string &name;
                const Luau::Module &checked;
                size_t caller;

                Collector(Graph &graph, const std::string &name, const Luau::Module &checked)
                    : graph(graph), name(name), checked(checked), caller(graph.module(name)) {}

                bool visit(Luau::AstExprFunction *node) override {
                    const auto previous = caller;
                    caller = graph.function(name, *node, checked);
                    node->body->visit(this);
                    caller = previous;

                    return false;
                }

                bool visit(Luau::AstExprCall *node) override {
                    const auto *type = checked.astOriginalCallTypes.find(node);
                    const auto *selected = checked.astOverloadResolvedTypes.find(node);

                    if (selected) {
                        const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*selected));

                        if (function && function->definition) {
                            type = selected;
                        }
                    }

                    if (!type) {
                        type = checked.astTypes.find(node->func);
                    }

                    if (type) {
                        graph.call(caller, *type, name, node->func->location);
                    }

                    return true;
                }
            };
        }

        NativeHierarchy hierarchy(
            AnalysisState &state, const NativeOperation &host, rust::Slice<const rust::String> modules,
            rust::Str module, rust::Slice<const uint32_t> position
        ) {
            if (!position.empty() && position.size() != 2) {
                throw std::runtime_error("invalid hierarchy position");
            }

            Graph graph(host, state.frontend.globals);

            for (const auto &module : modules) {
                const std::string name(module.data(), module.size());
                const auto *source = state.frontend.getSourceModule(name);
                const auto checked = state.frontend.moduleResolver.getModule(name);

                if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
                    throw std::runtime_error("semantic module is unavailable");
                }

                Collector collector(graph, name, *checked);
                source->root->visit(&collector);

                for (const auto &[range, scope] : checked->scopes) {
                    graph.scope(scope);
                }
            }

            if (!position.empty()) {
                const std::string name(module.data(), module.size());
                const auto *source = state.frontend.getSourceModule(name);
                const auto checked = state.frontend.moduleResolver.getModule(name);

                if (!source || !checked) {
                    throw std::runtime_error("hierarchy entry is unavailable");
                }

                if (const auto type =
                        inferred_type(state, *checked, *source, Luau::Position(position[0], position[1]))) {
                    graph.select(*type);
                }
            }

            return graph.finish();
        }
    }
}

namespace instar::native {
    namespace {
        NativeCompletion interruption(const NativeOperation &operation) {
            if (operation.requested()) {
                return NativeCompletion::Cancelled;
            }

            return operation.remaining_nanoseconds() == 0 ? NativeCompletion::Timeout : NativeCompletion::Complete;
        }

        rust::Vec<rust::String> inferred_modules(const AnalysisState &state) {
            rust::Vec<rust::String> result;

            for (const auto &module : state.captured.modules) {
                const std::string name(module.name);
                const auto checked = state.frontend.moduleResolver.getModule(name);

                if (state.outcomes.at(name).inferred && checked && checked->hasModuleScope()) {
                    result.push_back(module.name);
                }
            }

            return result;
        }

        const Luau::AstExpr *ungroup(const Luau::AstExpr *expression) {
            while (true) {
                if (const auto *group = expression->as<Luau::AstExprGroup>()) {
                    expression = group->expr;
                } else if (const auto *assertion = expression->as<Luau::AstExprTypeAssertion>()) {
                    expression = assertion->expr;
                } else {
                    return expression;
                }
            }
        }

        bool builtin_math(AnalysisState &state, const Luau::Module &checked, const Luau::AstExpr *expression) {
            const auto *global = ungroup(expression)->as<Luau::AstExprGlobal>();

            if (!global || std::string(global->name.value) != "math") {
                return false;
            }

            const auto scope = Luau::findScopeAtPosition(checked, global->location.begin);
            const auto binding = scope ? scope->lookupEx(Luau::Symbol(global->name)) : std::nullopt;

            return binding && binding->second == state.frontend.globals.globalScope.get();
        }

        bool builtin_abs(AnalysisState &state, const Luau::Module &checked, const Luau::AstExpr *expression) {
            const auto *member = ungroup(expression)->as<Luau::AstExprIndexName>();

            return member && member->op == '.' && std::string(member->index.value) == "abs" &&
                   builtin_math(state, checked, member->expr);
        }

        bool literal_abs(AnalysisState &state, const Luau::Module &checked, const Luau::AstExprCall *call) {
            if (call->self || call->args.size != 1 || !builtin_abs(state, checked, call->func)) {
                return false;
            }

            const auto *number = ungroup(call->args.data[0])->as<Luau::AstExprConstantNumber>();

            return number && std::isfinite(number->value);
        }

        struct StableBuiltin final : Luau::AstVisitor {
            AnalysisState &state;
            const Luau::Module &checked;
            bool stable = true;

            StableBuiltin(AnalysisState &state, const Luau::Module &checked) : state(state), checked(checked) {}

            bool visit(Luau::AstExpr *expression) override {
                if (expression->is<Luau::AstExprConstantNil>() || expression->is<Luau::AstExprConstantBool>() ||
                    expression->is<Luau::AstExprConstantNumber>() || expression->is<Luau::AstExprConstantString>() ||
                    expression->is<Luau::AstExprLocal>() || expression->is<Luau::AstExprGroup>() ||
                    expression->is<Luau::AstExprTypeAssertion>() || builtin_math(state, checked, expression) ||
                    builtin_abs(state, checked, expression)) {
                    return true;
                }

                const auto *call = expression->as<Luau::AstExprCall>();

                if (!call || !literal_abs(state, checked, call)) {
                    stable = false;
                }

                return stable;
            }

            bool visit(Luau::AstStatAssign *assignment) override {
                for (const auto *target : assignment->vars) {
                    if (!target->is<Luau::AstExprLocal>()) {
                        stable = false;
                    }
                }

                return stable;
            }

            bool visit(Luau::AstStatCompoundAssign *) override {
                stable = false;

                return false;
            }

            bool visit(Luau::AstStatTypeFunction *) override { return false; }
        };

        bool stable_builtins(AnalysisState &state) {
            for (const auto &module : state.captured.modules) {
                const std::string name(module.name);

                const auto &outcome = state.outcomes.at(name);

                if (!outcome.inferred || !outcome.graph_available) {
                    return false;
                }

                const auto checked = state.frontend.moduleResolver.getModule(name);
                const auto *source = state.frontend.getSourceModule(name);

                if (!checked || !source || !source->root || !source->parseErrors.empty()) {
                    return false;
                }

                StableBuiltin proof(state, *checked);
                source->root->visit(&proof);

                if (!proof.stable) {
                    return false;
                }
            }

            return true;
        }

        struct Expressions final : Luau::AstVisitor {
            AnalysisState &state;
            const NativeOperation &operation;
            const std::string &name;
            const Luau::Module *checked;
            NativeFactsResult &result;
            bool stable;
            std::set<const Luau::AstExpr *> writes;
            std::set<const Luau::AstExpr *> compound;

            Expressions(
                AnalysisState &state, const NativeOperation &operation, const std::string &name,
                const Luau::Module *checked, NativeFactsResult &result
            )
                : state(state), operation(operation), name(name), checked(checked), result(result),
                  stable(stable_builtins(state)) {}

            bool visit(Luau::AstExpr *expression) override {
                result.completion = interruption(operation);

                if (result.completion != NativeCompletion::Complete) {
                    return false;
                }

                NativeExpression observed{};
                observed.location = location(operation, name, expression->location);

                if (checked) {
                    if (const auto type = expression_type(state, *checked, expression)) {
                        observed.has_type = true;
                        observed.type_text = rust::String(Luau::toString(type));
                        observed.is_any = Luau::get<Luau::AnyType>(Luau::follow(type)) != nullptr;
                        types(operation, type, observed.type_declarations);
                    }
                }

                if (expression->is<Luau::AstExprConstantNil>()) {
                    observed.constant_kind = 1;
                    observed.literal = true;
                } else if (const auto *value = expression->as<Luau::AstExprConstantBool>()) {
                    observed.constant_kind = 2;
                    observed.constant_boolean = value->value;
                    observed.literal = true;
                } else if (const auto *value = expression->as<Luau::AstExprConstantNumber>()) {
                    if (std::isfinite(value->value)) {
                        observed.constant_kind = 3;
                        observed.constant_number = value->value;
                    }

                    observed.literal = true;
                } else if (const auto *value = expression->as<Luau::AstExprConstantString>()) {
                    observed.literal = true;

                    try {
                        observed.constant_text = rust::String(value->value.data, value->value.size);
                        observed.constant_kind = 4;
                    } catch (const std::invalid_argument &) {
                        observed.constant_kind = 0;
                    }
                } else if (const auto *value = expression->as<Luau::AstExprLocal>()) {
                    observed.dependency = location(operation, name, value->local->location);
                    observed.has_dependency = true;
                    observed.write = writes.count(expression) != 0;
                    observed.read = !observed.write || compound.count(expression) != 0;
                } else if (expression->is<Luau::AstExprFunction>() || expression->is<Luau::AstExprTable>()) {
                    observed.constant_kind = 5;
                }

                observed.known_effects = observed.literal || observed.has_dependency;

                if (checked && stable) {
                    if (builtin_math(state, *checked, expression)) {
                        observed.builtin_dependency = "math";
                        observed.known_effects = true;
                    } else if (const auto *call = expression->as<Luau::AstExprCall>();
                        builtin_abs(state, *checked, expression) || (call && literal_abs(state, *checked, call))) {
                        observed.builtin_dependency = "math.abs";
                        observed.known_effects = true;
                    }
                }

                if (const auto *call = expression->as<Luau::AstExprCall>()) {
                    NativeCallEvidence evidence{};
                    evidence.location = observed.location;
                    evidence.callee = location(operation, name, call->func->location);

                    for (const auto *argument : call->args) {
                        evidence.arguments.push_back(location(operation, name, argument->location));
                    }

                    evidence.argument_count = call->args.size;
                    evidence.has_argument_count = true;

                    if (call->args.size != 0) {
                        const auto *last = call->args.data[call->args.size - 1];

                        if (last->is<Luau::AstExprCall>() || last->is<Luau::AstExprVarargs>()) {
                            evidence.has_argument_count = false;

                            if (const auto *pack = checked ? checked->astTypePacks.find(last) : nullptr) {
                                const auto [head, tail] = Luau::flatten(*pack);

                                if (!tail) {
                                    evidence.argument_count = call->args.size - 1 + head.size();
                                    evidence.has_argument_count = true;
                                }
                            }
                        }
                    }

                    if (checked) {
                        auto position = call->argLocation.begin;
                        evidence.signatures = signatures(state, operation, name, position.line, position.column);
                    }

                    result.calls.push_back(std::move(evidence));
                }

                result.expressions.push_back(std::move(observed));

                return true;
            }

            bool visit(Luau::AstStatAssign *assignment) override {
                for (const auto *target : assignment->vars) {
                    writes.insert(target);
                }

                return true;
            }

            bool visit(Luau::AstStatCompoundAssign *assignment) override {
                writes.insert(assignment->var);
                compound.insert(assignment->var);

                return true;
            }

            bool visit(Luau::AstStatTypeFunction *) override { return false; }

            bool visit(Luau::AstType *) override { return true; }

            bool visit(Luau::AstTypePack *) override { return true; }
        };
    }

    NativeFactsResult facts(AnalysisState &state, const NativeFactsInput &input, const NativeOperation &operation) {
        NativeFactsResult result{};
        result.completion = interruption(operation);

        if (result.completion != NativeCompletion::Complete) {
            return result;
        }

        std::set<uint8_t> categories;

        for (const auto category : input.categories) {
            if (category > 10 || !categories.insert(category).second) {
                throw std::invalid_argument("invalid semantic category selection");
            }
        }

        const auto &captured = state.resolver.captured.at(std::string(input.name));

        for (const auto category : categories) {
            if (std::find(captured.retained.begin(), captured.retained.end(), category) == captured.retained.end()) {
                throw std::invalid_argument("semantic category was not retained by captured analysis");
            }
        }

        const std::string name(input.name);
        const auto *source = state.frontend.getSourceModule(name);

        if (!source || !source->root) {
            throw std::invalid_argument("captured native syntax is unavailable");
        }

        const auto &outcome = state.outcomes.at(name);
        auto checked = outcome.inferred ? state.frontend.moduleResolver.getModule(name) : nullptr;

        if (checked && !checked->hasModuleScope()) {
            checked = nullptr;
        }

        result.evidence_complete = checked && outcome.graph_available;

        if (checked) {
            rust::Vec<rust::String> current;
            current.push_back(input.name);
            result.references = references(state, operation, {current.data(), current.size()});
            const auto modules = inferred_modules(state);
            result.hierarchy = hierarchy(state, operation, {modules.data(), modules.size()}, "", {});

            for (const auto &[range, scope] : checked->scopes) {
                NativeScope record{location(operation, name, range), {}, false};

                if (scope->parent && source_scope(*checked, scope->parent.get())) {
                    record.parent = location(operation, name, scope->parent->location);
                    record.has_parent = true;
                }

                result.scopes.push_back(std::move(record));
            }

            std::set<std::pair<std::string, uint8_t>> names;

            for (auto scope = checked->getModuleScope()->parent; scope; scope = scope->parent) {
                for (const auto &[symbol, binding] : scope->bindings) {
                    if (!symbol.global.value) {
                        continue;
                    }

                    const std::string binding_name(symbol.global.value);

                    if (!names.emplace(binding_name, 4).second) {
                        continue;
                    }

                    NativeEnvironmentBinding record{};
                    record.name = rust::String(binding_name);
                    record.kind = 4;
                    record.type_text = rust::String(Luau::toString(binding.typeId));
                    record.is_any = Luau::get<Luau::AnyType>(Luau::follow(binding.typeId)) != nullptr;

                    record.documentation = rust::String(binding.documentationSymbol.value_or(
                        Luau::follow(binding.typeId)->documentationSymbol.value_or("")
                    ));

                    record.deprecated = binding.deprecated;
                    rust::Vec<NativeLocation> declarations;
                    declared(name, state, "global/" + binding_name, declarations);

                    if (!declarations.empty()) {
                        record.declaration = declarations[0];
                        record.has_declaration = true;
                    }

                    result.environment_bindings.push_back(std::move(record));
                }

                for (const auto &[binding_name, binding] : scope->exportedTypeBindings) {
                    if (!names.emplace(binding_name, 6).second) {
                        continue;
                    }

                    NativeEnvironmentBinding record{};
                    record.name = rust::String(binding_name);
                    record.kind = 6;
                    record.type_text = rust::String(Luau::toString(binding.type));
                    record.is_any = Luau::get<Luau::AnyType>(Luau::follow(binding.type)) != nullptr;
                    record.documentation = rust::String(Luau::follow(binding.type)->documentationSymbol.value_or(""));
                    rust::Vec<NativeLocation> declarations;
                    declared(name, state, "type/" + binding_name, declarations);

                    if (!declarations.empty()) {
                        record.declaration = declarations[0];
                        record.has_declaration = true;
                    }

                    result.environment_bindings.push_back(std::move(record));
                }
            }
        }

        Expressions visitor(state, operation, name, checked.get(), result);
        source->root->visit(&visitor);
        result.completion = interruption(operation);

        return result;
    }

    NativeInspectionResult
    inspect(AnalysisState &state, const NativePositionInput &input, const NativeOperation &operation) {
        NativeInspectionResult result{};
        result.completion = interruption(operation);

        if (result.completion != NativeCompletion::Complete) {
            return result;
        }

        if (operation.offset(input.name, input.line, input.column) != input.offset) {
            throw std::invalid_argument("native position disagrees with captured source coordinates");
        }

        result.hover = hover(state, operation, input.name, input.line, input.column);
        result.definitions = definition(state, operation, input.name, input.line, input.column, false);
        result.type_definitions = definition(state, operation, input.name, input.line, input.column, true);
        result.signatures = signatures(state, operation, input.name, input.line, input.column);
        const auto modules = inferred_modules(state);
        const uint32_t position[] = {input.line, input.column};
        result.hierarchy = hierarchy(state, operation, {modules.data(), modules.size()}, input.name, {position, 2});
        result.completion = interruption(operation);

        return result;
    }
}
