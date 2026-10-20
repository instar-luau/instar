#include "index.hpp"

#include "Luau/AstQuery.h"

#include <algorithm>
#include <map>
#include <utility>

namespace instar::luau {
    namespace {
        struct PropertyOrigin {
            const Luau::Property *property;
            std::string identity;
        };

        std::optional<PropertyOrigin> property(Luau::TypeId type, const std::string &name, unsigned depth = 0) {
            if (!type || depth > 64) {
                return std::nullopt;
            }

            type = Luau::follow(type);

            if (const auto *table = Luau::get<Luau::TableType>(type)) {
                if (const auto found = table->props.find(name); found != table->props.end()) {
                    return PropertyOrigin{&found->second, table->definitionModuleName};
                }
            } else if (const auto *external = Luau::get<Luau::ExternType>(type)) {
                if (const auto found = external->props.find(name); found != external->props.end()) {
                    return PropertyOrigin{&found->second, external->definitionModuleName};
                }

                if (external->parent) {
                    return property(*external->parent, name, depth + 1);
                }
            } else if (const auto *metatable = Luau::get<Luau::MetatableType>(type)) {
                if (auto found = property(metatable->table, name, depth + 1)) {
                    return found;
                }

                if (auto index = property(metatable->metatable, "__index", depth + 1)) {
                    if (index->property->readTy) {
                        return property(*index->property->readTy, name, depth + 1);
                    }
                }
            } else {
                const auto *intersection = Luau::get<Luau::IntersectionType>(type);
                const auto *union_type = Luau::get<Luau::UnionType>(type);
                const auto *parts = intersection ? &intersection->parts : (union_type ? &union_type->options : nullptr);
                std::optional<PropertyOrigin> result;

                if (parts) {
                    for (const auto part : *parts) {
                        const auto found = property(part, name, depth + 1);

                        if (!found) {
                            if (union_type) {
                                return std::nullopt;
                            }

                            continue;
                        }

                        if (result && (result->identity != found->identity ||
                                          result->property->location != found->property->location ||
                                          result->property->typeLocation != found->property->typeLocation)) {
                            return std::nullopt;
                        }

                        result = found;
                    }
                }

                return result;
            }

            return std::nullopt;
        }

        class Types final : public Luau::AstVisitor {
        public:
            bool visit(Luau::AstStatTypeAlias *node) override {
                declarations.push_back({node->name.value, node->location, node->nameLocation, node->exported});

                return true;
            }

            bool visit(Luau::AstStatTypeFunction *node) override {
                declarations.push_back({node->name.value, node->location, node->nameLocation, node->exported});

                return true;
            }

            bool visit(Luau::AstType *) override {
                return true;
            }

            std::optional<Luau::Location>
            find(const std::string &name, Luau::Location location, bool require_exported = false) const {
                for (const auto &declaration : declarations) {
                    if (declaration.name == name && (!require_exported || declaration.is_exported) &&
                        (location == declaration.statement || location == declaration.identifier)) {
                        return declaration.identifier;
                    }
                }

                return std::nullopt;
            }

        private:
            struct TypeDeclaration {
                std::string name;
                Luau::Location statement;
                Luau::Location identifier;
                bool is_exported;
            };

            std::vector<TypeDeclaration> declarations;
        };

        std::optional<Declaration> type_declaration(
            Luau::Frontend &frontend, const std::string &identity, std::optional<Luau::Location> location,
            const std::string &name, bool require_exported = false
        ) {
            const auto *source = frontend.getSourceModule(identity);

            if (!source || !source->root || !location) {
                return std::nullopt;
            }

            Types finder;
            source->root->visit(&finder);
            const auto identifier = finder.find(name, *location, require_exported);

            return identifier ? std::optional<Declaration>{{identity, *identifier}} : std::nullopt;
        }

        std::optional<Declaration>
        reference_declaration(Luau::Frontend &frontend, const Luau::Module &module, Luau::AstTypeReference *reference) {
            auto scope = Luau::findScopeAtPosition(module, reference->location.begin);

            if (reference->prefix) {
                while (scope) {
                    const auto imported = scope->importedModules.find(reference->prefix->value);

                    if (imported != scope->importedModules.end()) {
                        const auto target = frontend.moduleResolver.getModule(imported->second);
                        const auto *source = frontend.getSourceModule(imported->second);

                        if (target && source && source->root) {
                            const auto binding = target->exportedTypeBindings.find(reference->name.value);

                            if (binding != target->exportedTypeBindings.end()) {
                                return type_declaration(
                                    frontend,
                                    imported->second,
                                    binding->second.definitionLocation,
                                    reference->name.value,
                                    true
                                );
                            }
                        }

                        return std::nullopt;
                    }

                    scope = scope->parent;
                }
            } else {
                while (scope) {
                    const auto found = scope->typeAliasNameLocations.find(reference->name.value);

                    if (found != scope->typeAliasNameLocations.end()) {
                        return Declaration{module.name, found->second};
                    }

                    const auto binding = scope->privateTypeBindings.find(reference->name.value);

                    if (binding != scope->privateTypeBindings.end()) {
                        return type_declaration(
                            frontend,
                            module.name,
                            binding->second.definitionLocation,
                            reference->name.value
                        );
                    }

                    const auto exported = scope->exportedTypeBindings.find(reference->name.value);

                    if (exported != scope->exportedTypeBindings.end()) {
                        return type_declaration(
                            frontend,
                            module.name,
                            exported->second.definitionLocation,
                            reference->name.value
                        );
                    }

                    scope = scope->parent;
                }
            }

            return std::nullopt;
        }

        class Indexer final : public Luau::AstVisitor {
        public:
            Indexer(
                Index &result, Luau::Frontend &frontend, const Luau::Module &module, const Luau::SourceModule &source
            )
                : result(result), frontend(frontend), module(module) {
                source.root->visit(&types);
            }

            bool visit(Luau::AstType *) override {
                return true;
            }

            bool visit(Luau::AstTypePack *) override {
                return true;
            }

            bool visit(Luau::AstStatLocal *node) override {
                for (auto *local : node->vars) {
                    declare(local, node->isExported);
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *node) override {
                declare(node->name);

                return true;
            }

            bool visit(Luau::AstExprFunction *node) override {
                generics(node, node->generics, node->genericPacks);

                if (node->self) {
                    declare(node->self);
                }

                for (auto *local : node->args) {
                    declare(local);
                }

                return true;
            }

            bool visit(Luau::AstStatFor *node) override {
                declare(node->var);

                return true;
            }

            bool visit(Luau::AstStatForIn *node) override {
                for (auto *local : node->vars) {
                    declare(local);
                }

                return true;
            }

            bool visit(Luau::AstExprLocal *node) override {
                add_local(node->local, node->location, role(node));

                return true;
            }

            bool visit(Luau::AstExprGlobal *node) override {
                result.names.insert(node->name.value);
                auto scope = Luau::findScopeAtPosition(module, node->location.begin);
                const auto binding = scope ? scope->lookupEx(Luau::Symbol{node->name}) : std::nullopt;

                if (binding) {
                    if (const auto target = types.find(node->name.value, binding->first->location)) {
                        result.entries.push_back(
                            {node->location,
                                {module.name, *target},
                                OccurrenceRole::Type,
                                true,
                                BindingKind::Type,
                                node->name.value}
                        );

                        return true;
                    }

                    const bool is_owned =
                        std::any_of(module.scopes.begin(), module.scopes.end(), [&](const auto &entry) {
                            return entry.second.get() == binding->second;
                        });

                    auto target = definition(binding->first->typeId);

                    if (is_owned && binding->first->location.begin != binding->first->location.end) {
                        target = Declaration{module.name, binding->first->location};
                    } else if (target && !binding->first->location.encloses(target->location)) {
                        target.reset();
                    }

                    if (target) {
                        const auto access = target->identity == module.name && target->location == node->location
                                                ? OccurrenceRole::Declaration
                                                : role(node);

                        result.entries.push_back(
                            {node->location, *target, access, false, BindingKind::Global, node->name.value}
                        );
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatAssign *node) override {
                for (const auto *expression : node->vars) {
                    writes.insert(expression);
                }

                return true;
            }

            bool visit(Luau::AstStatCompoundAssign *node) override {
                updates.insert(node->var);

                return true;
            }

            bool visit(Luau::AstStatFunction *node) override {
                writes.insert(node->name);

                return true;
            }

            bool visit(Luau::AstExprIndexName *node) override {
                add_property(node->expr, node->index.value, node->indexLocation, role(node));

                return true;
            }

            bool visit(Luau::AstExprIndexExpr *node) override {
                if (const auto *key = node->index->as<Luau::AstExprConstantString>()) {
                    add_property(
                        node->expr,
                        std::string(key->value.data, key->value.size),
                        key_location(key),
                        role(node),
                        is_editable(key)
                    );
                }

                return true;
            }

            bool visit(Luau::AstExprTable *node) override {
                const auto *type = module.astTypes.find(node);

                if (type) {
                    for (const auto &item : node->items) {
                        if (item.key) {
                            if (const auto *key = item.key->as<Luau::AstExprConstantString>()) {
                                add_property(
                                    *type,
                                    std::string(key->value.data, key->value.size),
                                    key_location(key),
                                    OccurrenceRole::Declaration,
                                    is_editable(key)
                                );
                            }
                        }
                    }
                }

                return true;
            }

            bool visit(Luau::AstTypeTable *node) override {
                const auto *type = module.astResolvedTypes.find(node);

                if (type) {
                    for (const auto &item : node->props) {
                        add_property(*type, item.name.value, item.location, OccurrenceRole::Declaration);
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatTypeAlias *node) override {
                generics(node, node->generics, node->genericPacks);
                result.names.insert(node->name.value);

                result.entries.push_back(
                    {node->nameLocation,
                        {module.name, node->nameLocation},
                        OccurrenceRole::Declaration,
                        true,
                        BindingKind::Type,
                        node->name.value}
                );

                return true;
            }

            bool visit(Luau::AstStatTypeFunction *node) override {
                result.names.insert(node->name.value);

                result.entries.push_back(
                    {node->nameLocation,
                        {module.name, node->nameLocation},
                        OccurrenceRole::Declaration,
                        true,
                        BindingKind::Type,
                        node->name.value}
                );

                return true;
            }

            bool visit(Luau::AstGenericType *node) override {
                generic(node->name.value, node->location);

                return true;
            }

            bool visit(Luau::AstGenericTypePack *node) override {
                generic(node->name.value, node->location);

                return true;
            }

            bool visit(Luau::AstTypeFunction *node) override {
                generics(node, node->generics, node->genericPacks);

                return true;
            }

            bool visit(Luau::AstTypePackGeneric *node) override {
                result.names.insert(node->genericName.value);

                if (const auto target = generic_declaration(node->genericName.value, node->location.begin)) {
                    const Luau::Location location{node->location.begin,
                        static_cast<unsigned>(std::string(node->genericName.value).size())};

                    result.entries.push_back(
                        {location, *target, OccurrenceRole::Type, true, BindingKind::Type, node->genericName.value}
                    );
                }

                return true;
            }

            bool visit(Luau::AstTypeReference *node) override {
                result.names.insert(node->name.value);

                if (node->prefixLocal && node->prefixLocation) {
                    add_local(node->prefixLocal, *node->prefixLocation, OccurrenceRole::Read);
                }

                if (const auto target = reference_declaration(frontend, module, node)) {
                    result.entries.push_back(
                        {node->nameLocation, *target, OccurrenceRole::Type, true, BindingKind::Type, node->name.value}
                    );
                } else if (!node->prefix) {
                    if (const auto generic = generic_declaration(node->name.value, node->location.begin)) {
                        result.entries.push_back(
                            {node->nameLocation,
                                *generic,
                                OccurrenceRole::Type,
                                true,
                                BindingKind::Type,
                                node->name.value}
                        );
                    }
                }

                return true;
            }

            Index &result;

        private:
            void generic(const std::string &name, const Luau::Location &location) {
                result.names.insert(name);
                const Luau::Location identifier{location.begin, static_cast<unsigned>(name.size())};

                result.entries.push_back(
                    {identifier, {module.name, identifier}, OccurrenceRole::Declaration, true, BindingKind::Type, name}
                );
            }

            void generics(
                Luau::AstNode *node, Luau::AstArray<Luau::AstGenericType *> types,
                Luau::AstArray<Luau::AstGenericTypePack *> packs
            ) {
                if (const auto *scope = module.astScopes.find(node)) {
                    for (const auto *type : types) {
                        generic_locations[{*scope, type->name.value}] = {type->location.begin,
                            static_cast<unsigned>(std::string(type->name.value).size())};
                    }

                    for (const auto *pack : packs) {
                        generic_locations[{*scope, pack->name.value}] = {pack->location.begin,
                            static_cast<unsigned>(std::string(pack->name.value).size())};
                    }
                }
            }

            std::optional<Declaration> generic_declaration(const std::string &name, Luau::Position position) const {
                auto scope = Luau::findScopeAtPosition(module, position);

                while (scope) {
                    if (scope->privateTypeBindings.count(name) || scope->privateTypePackBindings.count(name)) {
                        const auto found = generic_locations.find({scope.get(), name});

                        if (found != generic_locations.end()) {
                            return Declaration{module.name, found->second};
                        }

                        return std::nullopt;
                    }

                    scope = scope->parent;
                }

                return std::nullopt;
            }

            void declare(Luau::AstLocal *local, bool is_exported = false) {
                result.names.insert(local->name.value);

                if (local->location.begin != local->location.end) {
                    result.entries.push_back(
                        {local->location,
                            {module.name, local->location},
                            OccurrenceRole::Declaration,
                            false,
                            BindingKind::Local,
                            local->name.value,
                            !is_exported}
                    );
                }
            }

            void add_local(Luau::AstLocal *local, const Luau::Location &location, OccurrenceRole role) {
                result.names.insert(local->name.value);

                if (local->location.begin != local->location.end) {
                    result.entries.push_back(
                        {location,
                            {module.name, local->location},
                            role,
                            false,
                            BindingKind::Local,
                            local->name.value,
                            !local->isExported}
                    );
                }
            }

            void add_property(
                Luau::AstExpr *expression, const std::string &name, const Luau::Location &location, OccurrenceRole role,
                bool is_editable = true
            ) {
                if (const auto *type = module.astTypes.find(expression)) {
                    add_property(*type, name, location, role, is_editable);
                }
            }

            void add_property(
                Luau::TypeId type, const std::string &name, const Luau::Location &location, OccurrenceRole role,
                bool is_editable = true
            ) {
                result.properties.insert(name);

                if (const auto found = property(type, name)) {
                    const auto target =
                        found->property->location ? found->property->location : found->property->typeLocation;

                    if (target && !found->identity.empty()) {
                        const auto access =
                            role == OccurrenceRole::Write && found->identity == module.name && *target == location
                                ? OccurrenceRole::Declaration
                                : role;

                        result.entries.push_back(
                            {location,
                                {found->identity, *target},
                                access,
                                false,
                                BindingKind::Property,
                                name,
                                is_editable}
                        );
                    }
                }
            }

            static bool is_editable(const Luau::AstExprConstantString *key) {
                return key->quoteStyle != Luau::AstExprConstantString::QuoteStyle::QuotedRaw &&
                       key->location.begin.line == key->location.end.line;
            }

            static Luau::Location key_location(const Luau::AstExprConstantString *key) {
                auto location = key->location;

                if (is_editable(key) && key->isQuoted()) {
                    ++location.begin.column;
                    --location.end.column;
                }

                return location;
            }

            OccurrenceRole role(const Luau::AstExpr *expression) const {
                if (updates.count(expression)) {
                    return OccurrenceRole::ReadWrite;
                }

                return writes.count(expression) ? OccurrenceRole::Write : OccurrenceRole::Read;
            }

            Luau::Frontend &frontend;
            const Luau::Module &module;
            Types types;
            std::unordered_set<const Luau::AstExpr *> writes;
            std::unordered_set<const Luau::AstExpr *> updates;
            std::map<std::pair<Luau::Scope *, std::string>, Luau::Location> generic_locations;
        };
    }

    bool is_same_declaration(const Declaration &left, const Declaration &right) {
        return left.identity == right.identity && left.location == right.location;
    }

    std::optional<Declaration> definition(Luau::TypeId type, unsigned depth) {
        if (!type || depth > 64) {
            return std::nullopt;
        }

        type = Luau::follow(type);

        if (const auto *function = Luau::get<Luau::FunctionType>(type)) {
            if (function->definition && function->definition->definitionModuleName) {
                const auto &value = *function->definition;

                const auto location = value.originalNameLocation.begin != value.originalNameLocation.end
                                          ? value.originalNameLocation
                                          : value.definitionLocation;

                return Declaration{*value.definitionModuleName, location};
            }
        } else if (const auto *table = Luau::get<Luau::TableType>(type)) {
            if (!table->definitionModuleName.empty()) {
                return Declaration{table->definitionModuleName, table->definitionLocation};
            }
        } else if (const auto *external = Luau::get<Luau::ExternType>(type)) {
            if (!external->definitionModuleName.empty() && external->definitionLocation) {
                return Declaration{external->definitionModuleName, *external->definitionLocation};
            }
        } else if (const auto *metatable = Luau::get<Luau::MetatableType>(type)) {
            return definition(metatable->table, depth + 1);
        }

        return std::nullopt;
    }

    Luau::TypeId local_type(const Luau::Module &module, Luau::AstLocal *local) {
        for (const auto &[location, scope] : module.scopes) {
            const auto found = scope->bindings.find(Luau::Symbol{local});

            if (found != scope->bindings.end()) {
                return found->second.typeId;
            }
        }

        return nullptr;
    }

    const Entry *at(const Index &index, Luau::Position position) {
        const Entry *result = nullptr;

        for (const auto &entry : index.entries) {
            if (entry.location.contains(position) && (!result || result->location.encloses(entry.location))) {
                result = &entry;
            }
        }

        return result;
    }

    std::optional<Declaration>
    annotation_declaration(Luau::Frontend &frontend, const Luau::Module &module, Luau::AstType *annotation) {
        if (!annotation) {
            return std::nullopt;
        }

        while (auto *group = annotation->as<Luau::AstTypeGroup>()) {
            annotation = group->type;
        }

        if (auto *reference = annotation->as<Luau::AstTypeReference>()) {
            return reference_declaration(frontend, module, reference);
        }

        return std::nullopt;
    }

    Index::Index(Luau::Frontend &frontend, const Luau::Module &module, const Luau::SourceModule &source) {
        Indexer visitor(*this, frontend, module, source);
        source.root->visit(&visitor);
    }

    Index index(Luau::Frontend &frontend, const Luau::Module &module, const Luau::SourceModule &source) {
        return Index{frontend, module, source};
    }
}
