#include "naming.hpp"

#include "Luau/AstQuery.h"
#include "Luau/ModuleResolver.h"
#include "types.hpp"

#include <algorithm>
#include <optional>
#include <set>
#include <string_view>
#include <type_traits>
#include <unordered_map>
#include <utility>

namespace instar::luau {
    namespace {
        class AliasReference final : public Luau::AstVisitor {
        public:
            AliasReference(Luau::Location location, PresentationBudget &budget) : location(location), budget(budget) {}

            bool visit(Luau::AstNode *node) override {
                budget.check();

                return node->location.contains(location.begin);
            }

            bool visit(Luau::AstType *node) override {
                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstTypePack *node) override {
                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstTypeReference *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                if (node->location == location) {
                    reference = node;
                }

                return true;
            }

            const Luau::AstTypeReference *reference = nullptr;

        private:
            Luau::Location location;
            PresentationBudget &budget;
        };

        class AliasArguments {
        public:
            AliasArguments(
                PresentationBudget &budget, Luau::TypeArena &arena, const PresentationLimits &limits,
                std::size_t &visits
            )
                : budget(budget), arena(arena), limits(limits), visits(visits) {}

            bool match(Luau::TypeId pattern, Luau::TypeId actual, std::size_t depth = 0) {
                budget.check();

                if (++visits > limits.edges || depth > limits.depth) {
                    return false;
                }

                pattern = Luau::follow(pattern, Luau::FollowOption::DisableLazyTypeThunks);
                actual = Luau::follow(actual, Luau::FollowOption::DisableLazyTypeThunks);

                if (const auto found = types.find(pattern); found != types.end()) {
                    if (!found->second) {
                        found->second = actual;
                    }

                    return found->second == actual;
                }

                if (pattern == actual || !seen_types.emplace(pattern, actual).second) {
                    return true;
                }

                return Luau::visit(
                    [&](const auto &value) {
                        using Value = std::decay_t<decltype(value)>;
                        const auto *other = Luau::get<Value>(actual);
                        if (!other) {
                            return false;
                        }
                        if constexpr (std::is_same_v<Value, Luau::MetatableType>) {
                            return match(value.table, other->table, depth + 1) &&
                                   match(value.metatable, other->metatable, depth + 1);
                        } else if constexpr (std::is_same_v<Value, Luau::TableType>) {
                            if (value.name != other->name ||
                                (value.name && (value.definitionModuleName != other->definitionModuleName ||
                                                   !match_sequence(
                                                       value.instantiatedTypeParams,
                                                       other->instantiatedTypeParams,
                                                       depth
                                                   ) ||
                                                   !match_sequence(
                                                       value.instantiatedTypePackParams,
                                                       other->instantiatedTypePackParams,
                                                       depth
                                                   ))) ||
                                value.props.size() != other->props.size() ||
                                bool(value.indexer) != bool(other->indexer)) {
                                return false;
                            }
                            for (const auto &[name, property] : value.props) {
                                budget.check();
                                const auto found = other->props.find(name);
                                if (found == other->props.end() ||
                                    !match_optional(property.readTy, found->second.readTy, depth) ||
                                    !match_optional(property.writeTy, found->second.writeTy, depth)) {
                                    return false;
                                }
                            }
                            return !value.indexer ||
                                   (value.indexer->isReadOnly == other->indexer->isReadOnly &&
                                       match(value.indexer->indexType, other->indexer->indexType, depth + 1) &&
                                       match(
                                           value.indexer->indexResultType,
                                           other->indexer->indexResultType,
                                           depth + 1
                                       ));
                        } else if constexpr (std::is_same_v<Value, Luau::FunctionType>) {
                            return value.generics == other->generics && value.genericPacks == other->genericPacks &&
                                   value.isCheckedFunction == other->isCheckedFunction &&
                                   match(value.argTypes, other->argTypes, depth + 1) &&
                                   match(value.retTypes, other->retTypes, depth + 1);
                        } else if constexpr (std::is_same_v<Value, Luau::UnionType>) {
                            return match_sequence(value.options, other->options, depth);
                        } else if constexpr (std::is_same_v<Value, Luau::IntersectionType>) {
                            return match_sequence(value.parts, other->parts, depth);
                        } else if constexpr (std::is_same_v<Value, Luau::NegationType>) {
                            return match(value.ty, other->ty, depth + 1);
                        } else if constexpr (std::is_same_v<Value, Luau::PrimitiveType>) {
                            return value.type == other->type;
                        } else if constexpr (std::is_same_v<Value, Luau::SingletonType>) {
                            return value == *other;
                        } else {
                            return false;
                        }
                    },
                    pattern->ty
                );
            }

            bool match(Luau::TypePackId pattern, Luau::TypePackId actual, std::size_t depth = 0) {
                budget.check();

                if (++visits > limits.edges || depth > limits.depth) {
                    return false;
                }

                pattern = Luau::follow(pattern);
                actual = Luau::follow(actual);

                if (const auto found = packs.find(pattern); found != packs.end()) {
                    if (!found->second) {
                        found->second = actual;

                        return true;
                    }

                    return found->second == actual || match(found->second, actual, depth + 1);
                }

                if (pattern == actual || !seen_packs.emplace(pattern, actual).second) {
                    return true;
                }

                if (const auto *variadic = Luau::get<Luau::VariadicTypePack>(pattern)) {
                    const auto *other = Luau::get<Luau::VariadicTypePack>(actual);

                    return other && variadic->hidden == other->hidden && match(variadic->ty, other->ty, depth + 1);
                }

                Luau::TypePack left;
                Luau::TypePack right;

                if (!flatten(pattern, left) || !flatten(actual, right) || left.head.size() > right.head.size()) {
                    return false;
                }

                for (std::size_t index = 0; index < left.head.size(); ++index) {
                    if (!match(left.head[index], right.head[index], depth + 1)) {
                        return false;
                    }
                }

                if (!left.tail) {
                    return left.head.size() == right.head.size() && !right.tail;
                }

                if (left.head.size() == right.head.size() && right.tail) {
                    return match(*left.tail, *right.tail, depth + 1);
                }

                right.head.erase(
                    right.head.begin(),
                    right.head.begin() + static_cast<std::vector<Luau::TypeId>::difference_type>(left.head.size())
                );

                return match(*left.tail, arena.addTypePack(std::move(right)), depth + 1);
            }

            std::unordered_map<Luau::TypeId, Luau::TypeId> types;
            std::unordered_map<Luau::TypePackId, Luau::TypePackId> packs;

        private:
            template <class Value>
            bool match_sequence(const std::vector<Value> &left, const std::vector<Value> &right, std::size_t depth) {
                if (left.size() != right.size()) {
                    return false;
                }

                for (std::size_t index = 0; index < left.size(); ++index) {
                    if (!match(left[index], right[index], depth + 1)) {
                        return false;
                    }
                }

                return true;
            }

            bool
            match_optional(std::optional<Luau::TypeId> left, std::optional<Luau::TypeId> right, std::size_t depth) {
                return bool(left) == bool(right) && (!left || match(*left, *right, depth + 1));
            }

            bool flatten(Luau::TypePackId value, Luau::TypePack &output) {
                while (const auto *pack = Luau::get<Luau::TypePack>(Luau::follow(value))) {
                    budget.check();
                    ++visits;

                    if (visits > limits.edges || pack->head.size() > limits.edges - visits) {
                        return false;
                    }

                    visits += pack->head.size();
                    output.head.insert(output.head.end(), pack->head.begin(), pack->head.end());

                    if (!pack->tail) {
                        return true;
                    }

                    value = *pack->tail;
                }

                output.tail = value;

                return true;
            }

            PresentationBudget &budget;
            Luau::TypeArena &arena;
            const PresentationLimits &limits;
            std::size_t &visits;
            std::set<std::pair<Luau::TypeId, Luau::TypeId>> seen_types;
            std::set<std::pair<Luau::TypePackId, Luau::TypePackId>> seen_packs;
        };
    }

    Naming::Naming(
        PresentationBudget &budget, Luau::ScopePtr scope, const Luau::Module &module, const Luau::SourceModule &source,
        const Luau::ModuleResolver &modules, const PresentationLimits &limits
    )
        : budget(budget), scope(std::move(scope)), module(module), source(source), modules(modules), limits(limits) {}

    Visibility Naming::table(const Luau::TableType &table) {
        Visibility result;

        if (!scope || !table.name || table.name->size() > limits.bytes) {
            return result;
        }

        const auto matches = [&](const std::optional<Luau::TypeFun> &binding) {
            const auto *original =
                binding
                    ? Luau::get<Luau::TableType>(Luau::follow(binding->type, Luau::FollowOption::DisableLazyTypeThunks))
                    : nullptr;

            return binding && binding->definitionLocation && original &&
                   original->definitionModuleName == table.definitionModuleName &&
                   original->definitionLocation == table.definitionLocation;
        };

        if (table.definitionModuleName == module.name) {
            if (matches(scope->lookupType(*table.name))) {
                result.is_verified = true;

                return result;
            }

            AliasReference selector(table.definitionLocation, budget);
            source.root->visit(&selector);
            const auto *reference = selector.reference;

            if (reference && reference->name.value == *table.name) {
                const auto origin_scope = Luau::findScopeAtPosition(module, table.definitionLocation.begin);
                const std::string_view spelling = reference->prefix ? reference->prefix->value : "";

                if (spelling.size() > limits.bytes) {
                    return result;
                }

                const std::string prefix(spelling);

                const auto original = origin_scope
                                          ? (reference->prefix ? origin_scope->lookupImportedType(prefix, *table.name)
                                                               : origin_scope->lookupType(*table.name))
                                          : std::nullopt;

                const auto visible =
                    reference->prefix ? scope->lookupImportedType(prefix, *table.name) : scope->lookupType(*table.name);

                const auto imported = [&](Luau::ScopePtr current) -> const std::string * {
                    while (current) {
                        budget.check();

                        if (const auto found = current->importedModules.find(prefix);
                            found != current->importedModules.end()) {
                            return &found->second;
                        }

                        current = current->parent;
                    }

                    return nullptr;
                };

                const auto *original_module = reference->prefix ? imported(origin_scope) : nullptr;
                const auto *visible_module = reference->prefix ? imported(scope) : nullptr;

                const bool is_same_module =
                    !reference->prefix || (original_module && visible_module && *original_module == *visible_module);

                if (prefix.size() <= limits.bytes && original && visible && original->definitionLocation &&
                    original->definitionLocation == visible->definitionLocation && is_same_module) {
                    result.is_verified = true;
                    result.qualifier = prefix;

                    return result;
                }
            }
        }

        std::optional<std::string> qualifier;

        for (auto current = scope; current; current = current->parent) {
            for (const auto &[prefix, identity] : current->importedModules) {
                budget.check();

                if (prefix.size() <= limits.bytes && (!qualifier || prefix < *qualifier) &&
                    matches(scope->lookupImportedType(prefix, *table.name))) {
                    qualifier = prefix;
                }
            }
        }

        if (qualifier) {
            result.is_verified = true;
            result.qualifier = *qualifier;

            return result;
        }

        return result;
    }

    TypeName Naming::metatable(const Luau::MetatableType &value, Luau::TypeId type, bool expand_structure) {
        TypeName result;

        const auto matches = [&](const std::optional<Luau::TypeFun> &binding) {
            if (!binding || !binding->definitionLocation || expand_structure) {
                return false;
            }

            AliasArguments candidate(budget, arena, limits, visits);

            for (const auto &parameter : binding->typeParams) {
                candidate.types.emplace(parameter.ty, nullptr);
            }

            for (const auto &parameter : binding->typePackParams) {
                candidate.packs.emplace(parameter.tp, nullptr);
            }

            if (!candidate.match(binding->type, type)) {
                return false;
            }

            std::vector<Luau::TypeId> type_arguments;
            std::vector<Luau::TypePackId> pack_arguments;

            for (const auto &parameter : binding->typeParams) {
                const auto found = candidate.types.at(parameter.ty);

                if (!found) {
                    return false;
                }

                type_arguments.push_back(found);
            }

            for (const auto &parameter : binding->typePackParams) {
                const auto found = candidate.packs.at(parameter.tp);

                if (!found) {
                    return false;
                }

                const auto *sequence = Luau::get<Luau::TypePack>(Luau::follow(found));

                if (binding->typePackParams.size() == 1 && sequence && !sequence->head.empty() && !sequence->tail) {
                    type_arguments.insert(type_arguments.end(), sequence->head.begin(), sequence->head.end());
                    continue;
                }

                pack_arguments.push_back(found);
            }

            result.type_arguments = std::move(type_arguments);
            result.pack_arguments = std::move(pack_arguments);

            return true;
        };

        bool is_preferred = false;
        std::size_t candidates = 0;
        auto lookup_scope = scope;

        const auto consider = [&](const std::string &spelling, const std::string &prefix) {
            budget.check();
            const bool is_exact = value.syntheticName && spelling == *value.syntheticName;

            if (++candidates > limits.edges || expand_structure || spelling.size() + prefix.size() > limits.bytes ||
                (result.is_verified &&
                    (is_preferred > is_exact || (is_preferred == is_exact && std::pair{result.qualifier, result.name} <=
                                                                                 std::pair{prefix, spelling})))) {
                return;
            }

            const auto binding = prefix.empty() ? lookup_scope->lookupType(spelling)
                                                : lookup_scope->lookupImportedType(prefix, spelling);

            if (!binding || !Luau::get<Luau::MetatableType>(
                                Luau::follow(binding->type, Luau::FollowOption::DisableLazyTypeThunks)
                            )) {
                return;
            }

            if (matches(binding)) {
                result.is_verified = true;
                is_preferred = is_exact;
                result.name = spelling;
                result.qualifier = prefix;
            }
        };

        const auto scan = [&](const Luau::ScopePtr &origin) {
            lookup_scope = origin;

            for (auto current_scope = origin; current_scope && !expand_structure;
                current_scope = current_scope->parent) {
                for (const auto &[spelling, binding] : current_scope->privateTypeBindings) {
                    consider(spelling, {});
                }

                for (const auto &[spelling, binding] : current_scope->exportedTypeBindings) {
                    consider(spelling, {});
                }

                for (const auto &[prefix, bindings] : current_scope->importedTypeBindings) {
                    for (const auto &[spelling, binding] : bindings) {
                        consider(spelling, prefix);
                    }
                }
            }
        };

        scan(scope);

        if (!result.is_verified && !expand_structure) {
            const auto *table =
                Luau::get<Luau::TableType>(Luau::follow(value.table, Luau::FollowOption::DisableLazyTypeThunks));

            if (table && !table->definitionModuleName.empty()) {
                if (const auto origin = modules.getModule(table->definitionModuleName)) {
                    scan(origin->getModuleScope());
                }
            }
        }

        return result;
    }

    bool has_ambiguous_name(Luau::TypeId value, const std::vector<Luau::TypeId> &alternatives) {
        const auto *table = Luau::get<Luau::TableType>(value);

        return table && table->name && std::any_of(alternatives.begin(), alternatives.end(), [&](Luau::TypeId other) {
            const auto *alternative = Luau::get<Luau::TableType>(other);

            return other != value && alternative && alternative->name == table->name;
        });
    }
}
