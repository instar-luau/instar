#include "hover.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Lexer.h"
#include "Luau/StringUtils.h"
#include "Luau/TypePack.h"
#include "query.hpp"

#include <algorithm>
#include <cstdint>
#include <optional>
#include <unordered_set>
#include <utility>

namespace instar::luau::query {
    namespace {
        enum class HoverKind : std::uint8_t {
            Local,
            Global,
            Alias,
            Field,
            Type,
            LocalFunction,
            Function,
            Parameter,
            Loop,
            Constant,
            ConstantFunction,
        };

        struct Target {
            Luau::TypeId type = nullptr;
            Luau::TypeId declared_type = nullptr;
            Luau::TypePackId pack = nullptr;
            Luau::Location location;
            std::string symbol;
            std::string documentation_symbol;
            bool is_alias = false;
            std::string qualifier;
            std::vector<Luau::TypeId> type_arguments;
            std::vector<Luau::TypePackId> pack_arguments;
            HoverKind kind = HoverKind::Type;

            Target(
                Luau::TypeId type, Luau::TypePackId pack, Luau::Location location, std::string symbol = {},
                HoverKind kind = HoverKind::Type
            )
                : type(type), pack(pack), location(location), symbol(std::move(symbol)), kind(kind) {}
        };

        std::string documented_name(const std::string &symbol) {
            for (const std::string prefix : {"/global/", "/globaltype/"}) {
                if (const auto start = symbol.find(prefix); start != std::string::npos) {
                    const auto begin = start + prefix.size();
                    const auto end = symbol.find('/', begin);

                    if (end != std::string::npos && symbol.find("/overload/", end) != end) {
                        return {};
                    }

                    return symbol.substr(begin, end - begin);
                }
            }

            return {};
        }

        void property(Target &target, const Luau::Module &module, Luau::AstExpr *receiver, PresentationBudget &budget) {
            struct Lookup {
                Luau::TypeId type;
                bool is_metatable;
            };

            const auto *root = module.astTypes.find(receiver);

            if (!root) {
                return;
            }

            std::vector<Lookup> pending{{*root, false}};
            std::unordered_set<Luau::TypeId> direct_seen;
            std::unordered_set<Luau::TypeId> metatable_seen;

            while (!pending.empty()) {
                budget.check();
                const auto current = pending.back();
                pending.pop_back();
                auto &seen = current.is_metatable ? metatable_seen : direct_seen;

                if (!current.type || !seen.insert(current.type).second) {
                    continue;
                }

                const Luau::TableType::Props *properties = nullptr;

                if (const auto *bound = Luau::get_if<Luau::BoundType>(&current.type->ty)) {
                    pending.push_back({bound->boundTo, current.is_metatable});
                } else if (const auto *table = Luau::get<Luau::TableType>(current.type)) {
                    if (table->boundTo) {
                        pending.push_back({*table->boundTo, current.is_metatable});
                    } else {
                        properties = &table->props;
                    }
                } else if (const auto *external = Luau::get<Luau::ExternType>(current.type)) {
                    properties = &external->props;

                    if (external->parent) {
                        pending.push_back({*external->parent, current.is_metatable});
                    }
                } else if (const auto *metatable = Luau::get<Luau::MetatableType>(current.type)) {
                    pending.push_back({metatable->metatable, true});
                    pending.push_back({metatable->table, current.is_metatable});
                } else if (const auto *primitive = Luau::get<Luau::PrimitiveType>(current.type);
                    primitive && primitive->metatable) {
                    pending.push_back({*primitive->metatable, true});
                } else if (const auto *lazy = Luau::get<Luau::LazyType>(current.type)) {
                    if (const auto value = lazy->unwrapped.load()) {
                        pending.push_back({value, current.is_metatable});
                    }
                }

                if (properties) {
                    const auto found = properties->find(current.is_metatable ? "__index" : target.symbol);

                    if (found == properties->end()) {
                        continue;
                    }

                    if (!current.is_metatable) {
                        target.declared_type = found->second.readTy.value_or(found->second.writeTy.value_or(nullptr));

                        if (const auto &symbol = found->second.documentationSymbol) {
                            target.documentation_symbol = *symbol;
                        }

                        if (const auto *external = Luau::get<Luau::ExternType>(current.type)) {
                            target.qualifier = external->name;
                        } else if (const auto *table = Luau::get<Luau::TableType>(current.type)) {
                            target.qualifier = table->name.value_or("");
                        }

                        if (const auto &symbol = current.type->documentationSymbol) {
                            auto name = documented_name(*symbol);

                            if (!name.empty()) {
                                target.qualifier = std::move(name);
                            }
                        }

                        return;
                    }

                    const auto type = found->second.readTy ? found->second.readTy : found->second.writeTy;

                    if (type) {
                        pending.push_back({*type, false});
                    }
                }
            }
        }

        class Declaration final : public Luau::AstVisitor {
        public:
            Declaration(const Luau::AstLocal *local, PresentationBudget &budget) : local(local), budget(budget) {}

            bool visit(Luau::AstNode *node) override {
                budget.check();

                return node->location.contains(local->location.begin);
            }

            bool visit(Luau::AstStatLocalFunction *node) override {
                if (node->name == local) {
                    kind = node->isConst ? HoverKind::ConstantFunction : HoverKind::LocalFunction;
                }

                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstExprFunction *node) override {
                if (node->self == local) {
                    kind = HoverKind::Parameter;
                }

                for (const auto *argument : node->args) {
                    budget.check();

                    if (argument == local) {
                        kind = HoverKind::Parameter;
                    }
                }

                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstStatFor *node) override {
                if (node->var == local) {
                    kind = HoverKind::Loop;
                }

                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstStatForIn *node) override {
                for (const auto *variable : node->vars) {
                    budget.check();

                    if (variable == local) {
                        kind = HoverKind::Loop;
                    }
                }

                return visit(static_cast<Luau::AstNode *>(node));
            }

            HoverKind kind = HoverKind::Local;

        private:
            const Luau::AstLocal *local;
            PresentationBudget &budget;
        };

        class Selection final : public Luau::AstVisitor {
        public:
            Selection(const Prepared &prepared, Luau::Position position, PresentationBudget &budget)
                : prepared(prepared), module(prepared.module), position(position), budget(budget) {}

            bool visit(Luau::AstNode *node) override {
                budget.check();

                return node->location.contains(position);
            }

            bool visit(Luau::AstType *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                if (node->is<Luau::AstTypeSingletonBool>() || node->is<Luau::AstTypeSingletonString>()) {
                    accept(annotation(node, node->location));
                }

                return true;
            }

            bool visit(Luau::AstTypePack *node) override {
                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstExpr *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                if (node->is<Luau::AstExprLocal>() || node->is<Luau::AstExprGlobal>() ||
                    node->is<Luau::AstExprConstantNil>() || node->is<Luau::AstExprConstantBool>() ||
                    node->is<Luau::AstExprConstantNumber>() || node->is<Luau::AstExprConstantInteger>() ||
                    node->is<Luau::AstExprConstantString>()) {
                    const auto *found = module.astTypes.find(node);
                    const auto name = Luau::getIdentifier(node);

                    if ((node->is<Luau::AstExprLocal>() || node->is<Luau::AstExprGlobal>()) &&
                        (!name.value || !is_identifier(node->location, name.value))) {
                        return false;
                    }

                    auto type = found ? *found : nullptr;

                    if (const auto *local = node->as<Luau::AstExprLocal>(); local && !type) {
                        type = local_type(module, local->local);
                    }

                    Target candidate{type,
                        nullptr,
                        node->location,
                        name.value ? name.value : "",
                        node->is<Luau::AstExprLocal>() ? binding_kind(node->as<Luau::AstExprLocal>()->local)
                        : node->is<Luau::AstExprGlobal>()
                            ? (type && Luau::get<Luau::FunctionType>(Luau::follow(type)) ? HoverKind::Function
                                                                                         : HoverKind::Global)
                            : HoverKind::Type};

                    if (node->is<Luau::AstExprLocal>() || node->is<Luau::AstExprGlobal>()) {
                        candidate.documentation_symbol = documentation(prepared.source, module, position, budget);
                    }

                    accept(std::move(candidate));
                }

                return true;
            }

            bool visit(Luau::AstExprVarargs *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                const auto *pack = module.astTypePacks.find(node);
                const auto *type = module.astTypes.find(node);
                accept({pack ? nullptr : type ? *type : nullptr, pack ? *pack : nullptr, node->location});

                return false;
            }

            bool visit(Luau::AstExprIndexName *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                if (is_identifier(node->indexLocation, node->index.value)) {
                    const auto *found = module.astTypes.find(node);

                    Target candidate{found ? *found : nullptr,
                        nullptr,
                        node->indexLocation,
                        node->index.value,
                        HoverKind::Field};

                    candidate.documentation_symbol = documentation(prepared.source, module, position, budget);
                    property(candidate, module, node->expr, budget);
                    accept(std::move(candidate));

                    return false;
                }

                return true;
            }

            bool visit(Luau::AstExprIndexExpr *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                if (const auto *key = node->index->as<Luau::AstExprConstantString>();
                    key && key->location.contains(position)) {
                    const auto *found = module.astTypes.find(node);

                    Target candidate{found ? *found : nullptr,
                        nullptr,
                        key->location,
                        std::string(key->value.data, key->value.size),
                        HoverKind::Field};

                    property(candidate, module, node->expr, budget);

                    accept(std::move(candidate));

                    return false;
                }

                return true;
            }

            bool visit(Luau::AstExprTable *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (const auto &item : node->items) {
                    budget.check();
                    const auto *key = item.key ? item.key->as<Luau::AstExprConstantString>() : nullptr;

                    if (key && key->location.contains(position)) {
                        const auto *found = module.astTypes.find(item.value);

                        accept(
                            {found ? *found : nullptr,
                                nullptr,
                                key->location,
                                std::string(key->value.data, key->value.size),
                                HoverKind::Field}
                        );

                        return false;
                    }
                }

                return true;
            }

            bool visit(Luau::AstTypeReference *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                if (is_identifier(node->nameLocation, node->name.value)) {
                    auto candidate = annotation(node, node->nameLocation, node->name.value);
                    const auto scope = Luau::findScopeAtPosition(module, position);

                    const auto binding =
                        scope ? (node->prefix ? scope->lookupImportedType(node->prefix->value, node->name.value)
                                              : scope->lookupType(node->name.value))
                              : std::nullopt;

                    if (binding && binding->definitionLocation) {
                        candidate.is_alias = true;
                        candidate.qualifier = node->prefix ? node->prefix->value : "";

                        for (const auto &parameter : node->parameters) {
                            budget.check();

                            if (parameter.type) {
                                const auto *resolved = module.astResolvedTypes.find(parameter.type);
                                candidate.type_arguments.push_back(resolved ? *resolved : nullptr);
                            } else {
                                const auto *resolved =
                                    parameter.typePack ? module.astResolvedTypePacks.find(parameter.typePack) : nullptr;

                                candidate.pack_arguments.push_back(resolved ? *resolved : nullptr);
                            }
                        }
                    }

                    accept(std::move(candidate));

                    return false;
                }

                if (node->prefixLocation && node->prefix && is_identifier(*node->prefixLocation, node->prefix->value)) {
                    auto type = node->prefixLocal ? local_type(module, node->prefixLocal) : nullptr;

                    if (!type && node->prefix) {
                        const auto scope = Luau::findScopeAtPosition(module, position);
                        const auto binding = scope ? scope->linearSearchForBinding(node->prefix->value) : std::nullopt;

                        if (binding) {
                            type = binding->typeId;
                        }
                    }

                    Target candidate{type,
                        nullptr,
                        *node->prefixLocation,
                        node->prefix ? node->prefix->value : "",
                        node->prefixLocal ? binding_kind(node->prefixLocal) : HoverKind::Global};

                    candidate.documentation_symbol = documentation(type, budget);
                    accept(std::move(candidate));

                    return false;
                }

                return true;
            }

            bool visit(Luau::AstTypeTable *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (const auto &property : node->props) {
                    budget.check();

                    if (property.location.contains(position)) {
                        auto candidate = annotation(property.type, property.location, property.name.value);
                        candidate.kind = HoverKind::Field;
                        const auto *table_type = module.astResolvedTypes.find(node);

                        if (table_type) {
                            const auto *table = Luau::get<Luau::TableType>(
                                Luau::follow(*table_type, Luau::FollowOption::DisableLazyTypeThunks)
                            );

                            if (table) {
                                const auto found = table->props.find(property.name.value);

                                if (found != table->props.end()) {
                                    if (const auto &symbol = found->second.documentationSymbol) {
                                        candidate.documentation_symbol = *symbol;
                                    }
                                }
                            }
                        }

                        accept(std::move(candidate));

                        return false;
                    }
                }

                return true;
            }

            bool visit(Luau::AstTypeFunction *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                generics(node, node->generics, node->genericPacks);

                for (std::size_t index = 0; index < node->argNames.size && index < node->argTypes.types.size; ++index) {
                    budget.check();
                    const auto &name = node->argNames.data[index];

                    if (name && is_identifier(name->second, name->first.value)) {
                        auto candidate = annotation(node->argTypes.types.data[index], name->second, name->first.value);
                        candidate.kind = HoverKind::Field;
                        accept(std::move(candidate));

                        return false;
                    }
                }

                return true;
            }

            bool visit(Luau::AstTypePackGeneric *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                const Luau::Location name{node->location.begin,
                    static_cast<unsigned>(std::string(node->genericName.value).size())};

                if (is_identifier(name, node->genericName.value)) {
                    const auto *found = module.astResolvedTypePacks.find(node);
                    accept({nullptr, found ? *found : nullptr, name, node->genericName.value});
                }

                return false;
            }

            bool visit(Luau::AstTypePackVariadic *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                const Luau::Location marker{node->location.begin, 3};

                if (marker.contains(position)) {
                    const auto *found = module.astResolvedTypePacks.find(node);
                    accept({nullptr, found ? *found : nullptr, marker});

                    return false;
                }

                return true;
            }

            bool visit(Luau::AstStatLocal *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (auto *local : node->vars) {
                    binding(local);
                }

                return true;
            }

            bool visit(Luau::AstStatFunction *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                if (const auto *name = node->name->as<Luau::AstExprGlobal>();
                    name && is_identifier(name->location, name->name.value)) {
                    const auto *found = module.astTypes.find(node->func);

                    Target candidate{found ? *found : nullptr,
                        nullptr,
                        name->location,
                        name->name.value,
                        HoverKind::Function};

                    candidate.documentation_symbol = documentation(prepared.source, module, position, budget);
                    accept(std::move(candidate));

                    return false;
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                binding(node->name);

                return true;
            }

            bool visit(Luau::AstStatFor *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                binding(node->var);

                return true;
            }

            bool visit(Luau::AstStatForIn *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (auto *local : node->vars) {
                    binding(local);
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (auto *local : node->args) {
                    binding(local);
                }

                generics(node, node->generics, node->genericPacks);

                return true;
            }

            bool visit(Luau::AstStatTypeAlias *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                generics(node, node->generics, node->genericPacks);

                if (is_identifier(node->nameLocation, node->name.value)) {
                    const auto scope = Luau::findScopeAtPosition(module, position);
                    const auto binding = scope ? scope->lookupType(node->name.value) : std::nullopt;

                    Target candidate{binding ? binding->type : nullptr,
                        nullptr,
                        node->nameLocation,
                        node->name.value,
                        HoverKind::Alias};

                    if (binding && binding->definitionLocation && *binding->definitionLocation == node->location) {
                        candidate.is_alias = true;

                        for (const auto &parameter : binding->typeParams) {
                            budget.check();
                            candidate.type_arguments.push_back(parameter.ty);
                        }

                        for (const auto &parameter : binding->typePackParams) {
                            budget.check();
                            candidate.pack_arguments.push_back(parameter.tp);
                        }
                    }

                    accept(std::move(candidate));

                    return false;
                }

                return true;
            }

            const std::optional<Target> &target() const {
                return result;
            }

        private:
            bool is_identifier(Luau::Location location, const std::string &name) const {
                if (!location.contains(position)) {
                    return false;
                }

                const auto begin = offset(prepared.text, location.begin);
                const auto end = offset(prepared.text, location.end);

                return end - begin == name.size() && prepared.text.compare(begin, end - begin, name) == 0;
            }

            void accept(Target candidate) {
                if (!candidate.location.contains(position)) {
                    return;
                }

                if (result && !(result->location.begin <= candidate.location.begin &&
                                  candidate.location.end <= result->location.end)) {
                    return;
                }

                if (candidate.documentation_symbol.empty()) {
                    candidate.documentation_symbol = documentation(candidate.type, budget);
                }

                if (candidate.documentation_symbol.empty()) {
                    candidate.documentation_symbol = documentation(candidate.declared_type, budget);
                }

                result = std::move(candidate);
            }

            Target annotation(Luau::AstType *node, Luau::Location location, const std::string &symbol = {}) {
                const auto *found = module.astResolvedTypes.find(node);

                return {found ? *found : nullptr, nullptr, location, symbol};
            }

            HoverKind binding_kind(const Luau::AstLocal *local) {
                Declaration declaration(local, budget);
                prepared.source.root->visit(&declaration);

                return declaration.kind == HoverKind::Local && local->isConst ? HoverKind::Constant : declaration.kind;
            }

            void binding(Luau::AstLocal *local) {
                budget.check();

                if (is_identifier(local->location, local->name.value)) {
                    Target candidate{local_type(module, local),
                        nullptr,
                        local->location,
                        local->name.value,
                        binding_kind(local)};

                    candidate.documentation_symbol = documentation(prepared.source, module, position, budget);
                    accept(std::move(candidate));
                }
            }

            void generics(
                Luau::AstNode *owner, Luau::AstArray<Luau::AstGenericType *> types,
                Luau::AstArray<Luau::AstGenericTypePack *> packs
            ) {
                const auto *scope = module.astScopes.find(owner);

                for (auto *generic : types) {
                    budget.check();

                    const Luau::Location name{generic->location.begin,
                        static_cast<unsigned>(std::string(generic->name.value).size())};

                    if (is_identifier(name, generic->name.value)) {
                        const auto binding = scope ? (*scope)->lookupType(generic->name.value) : std::nullopt;
                        accept({binding ? binding->type : nullptr, nullptr, name, generic->name.value});
                    }

                    if (generic->defaultValue) {
                        generic->defaultValue->visit(this);
                    }
                }

                for (auto *generic : packs) {
                    budget.check();

                    const Luau::Location name{generic->location.begin,
                        static_cast<unsigned>(std::string(generic->name.value).size())};

                    if (is_identifier(name, generic->name.value)) {
                        const auto binding = scope ? (*scope)->lookupPack(generic->name.value) : std::nullopt;
                        accept({nullptr, binding ? *binding : nullptr, name, generic->name.value});
                    }

                    if (generic->defaultValue) {
                        generic->defaultValue->visit(this);
                    }
                }
            }

            std::optional<Target> result;
            const Prepared &prepared;
            const Luau::Module &module;
            Luau::Position position;
            PresentationBudget &budget;
        };
    }

    rust::Vec<NativeHover> hover(
        Luau::Frontend &, const Host &host, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget
    ) {
        rust::Vec<NativeHover> result;
        offset(prepared.text, position);

        if (Luau::isWithinComment(prepared.source, position)) {
            return result;
        }

        Selection selector(prepared, position, budget);
        prepared.source.root->visit(&selector);
        const auto &target = selector.target();

        if (!target || (!target->type && !target->pack)) {
            return result;
        }

        Printer printer(budget, Luau::findScopeAtPosition(prepared.module, position));

        const auto type =
            target->type ? Luau::follow(target->type, Luau::FollowOption::DisableLazyTypeThunks) : nullptr;

        const auto name = documented_name(target->documentation_symbol);

        const bool is_namespace = type &&
                                  (Luau::get<Luau::TableType>(type) ||
                                      (Luau::get<Luau::ExternType>(type) && Luau::hasTag(type, "instar.namespace"))) &&
                                  !name.empty() && target->documentation_symbol.find("/global/") != std::string::npos;

        const bool is_binding = target->kind == HoverKind::Local || target->kind == HoverKind::Constant;

        const bool is_qualified_field = target->kind == HoverKind::Field && !target->qualifier.empty() &&
                                        Luau::isIdentifier(target->symbol) && !Luau::Lexer::isReserved(target->symbol);

        const auto functions = overloads(type, budget);

        if (!target->is_alias && !target->symbol.empty() && !functions.empty()) {
            auto callable = functions.front();

            for (const auto *node : Luau::findAstAncestryOfPosition(prepared.source, position)) {
                if (const auto *call = node->as<Luau::AstExprCall>(); call && call->func->location.contains(position)) {
                    if (const auto *resolved = prepared.module.astOverloadResolvedTypes.find(call)) {
                        const auto selected = Luau::follow(*resolved, Luau::FollowOption::DisableLazyTypeThunks);

                        if (std::find(functions.begin(), functions.end(), selected) != functions.end()) {
                            callable = selected;
                        }
                    }
                }
            }

            if (is_binding) {
                const auto binding =
                    (target->kind == HoverKind::Constant ? "const " : "local ") + target->symbol +
                    (!name.empty() && target->documentation_symbol.find("/global/") != std::string::npos
                            ? ": typeof(" + name + ")"
                            : "");

                result.push_back({locate(host, prepared.module.name, target->location), binding, {}, {}, false, false});
            }

            budget.check();
            const auto *function = Luau::get<Luau::FunctionType>(callable);

            auto qualified =
                name.empty() ? (is_qualified_field ? target->qualifier + "." + target->symbol : target->symbol) : name;

            if (function->hasSelf) {
                if (const auto separator = qualified.rfind('.'); separator != std::string::npos) {
                    const auto receiver = Luau::first(function->argTypes);

                    const auto type =
                        receiver ? Luau::follow(*receiver, Luau::FollowOption::DisableLazyTypeThunks) : nullptr;

                    if (type && Luau::get<Luau::ExternType>(type) && Luau::hasTag(type, "instar.namespace")) {
                        const auto owner = documented_name(documentation(type, budget));

                        if (!owner.empty()) {
                            qualified.replace(0, separator, owner);
                        }
                    }

                    qualified[qualified.rfind('.')] = ':';
                }
            }

            const std::string prefix = target->kind == HoverKind::LocalFunction      ? "local function "
                                       : target->kind == HoverKind::ConstantFunction ? "const function "
                                                                                     : "function ";

            result.push_back(
                {locate(host, prepared.module.name, target->location),
                    printer.finish(
                        (function->isCheckedFunction ? "@checked " : "") + prefix +
                        printer.function(*function, qualified, true)
                    ),
                    target->documentation_symbol,
                    printer.signature(callable),
                    false,
                    function->hasSelf && qualified.find(':') != std::string::npos}
            );

            return result;
        }

        const auto describe = [&] {
            if (target->is_alias) {
                std::string name =
                    target->qualifier.empty() ? target->symbol : target->qualifier + "." + target->symbol;

                bool first = true;

                const auto argument = [&](const std::string &text) {
                    name += first ? "<" : ", ";
                    name += text;
                    first = false;
                };

                for (const auto type : target->type_arguments) {
                    argument(printer.type(type));
                }

                for (const auto pack : target->pack_arguments) {
                    argument(printer.pack(pack));
                }

                if (!first) {
                    name += '>';
                }

                return printer.finish("type " + name + " = " + printer.type(target->type, true));
            }

            if (is_namespace) {
                return is_binding ? (target->kind == HoverKind::Constant ? "const " : "local ") + target->symbol +
                                        ": typeof(" + name + ")"
                                  : name;
            }

            auto text = target->pack ? printer.pack(target->pack, true) : printer.type(target->type);

            if (target->kind == HoverKind::Local || target->kind == HoverKind::Constant) {
                text = (target->kind == HoverKind::Constant ? "const " : "local ") + target->symbol + ": " + text;
            } else if (target->kind == HoverKind::Field) {
                text = target->qualifier +
                       (is_qualified_field ? "." + target->symbol : "[\"" + Luau::escape(target->symbol) + "\"]") +
                       ": " + text;
            } else if (target->kind != HoverKind::Type && !target->symbol.empty()) {
                text = target->symbol + ": " + text;
            }

            return printer.finish(std::move(text));
        };

        result.push_back(
            {locate(host, prepared.module.name, target->location),
                describe(),
                target->documentation_symbol,
                {},
                is_namespace && !is_binding && !target->is_alias,
                false}
        );

        return result;
    }
}
