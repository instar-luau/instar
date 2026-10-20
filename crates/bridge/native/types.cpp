#include "types.hpp"

#include "Luau/TypeFunction.h"
#include "naming.hpp"
#include "reduction.hpp"

#include <cstdint>
#include <limits>
#include <stdexcept>
#include <string_view>
#include <type_traits>
#include <unordered_map>
#include <utility>
#include <vector>

namespace instar::luau {
    namespace {
        template <class> constexpr bool is_unsupported = false;
        constexpr auto absent = std::numeric_limits<std::size_t>::max();

        struct Work {
            Luau::TypeId type = nullptr;
            Luau::TypePackId pack = nullptr;
            std::size_t index;
            std::size_t depth;
            bool expand_structure = false;
        };
    }

    PresentationLimits::PresentationLimits(bool is_expanded)
        : nodes(is_expanded ? 2048 : 256), edges(is_expanded ? 16384 : 1024), depth(is_expanded ? 64 : 8),
          bytes(is_expanded ? 262144 : 16384) {}

    PresentationBudget::PresentationBudget(std::shared_ptr<Luau::FrontendCancellationToken> cancellation)
        : cancellation(std::move(cancellation)) {}

    void PresentationBudget::check() const {
        if (cancellation && cancellation->requested()) {
            throw std::runtime_error("Analysis interrupted.");
        }
    }

    class Extraction::Traversal {
    public:
        Traversal(
            PresentationBudget &budget, bool is_expanded, Luau::ScopePtr scope, const Luau::Module &module,
            const Luau::SourceModule &source, Luau::BuiltinTypes &builtins, const Luau::ModuleResolver &modules
        )
            : budget(budget), builtins(builtins), limits(is_expanded),
              naming(budget, std::move(scope), module, source, modules, limits) {
            snapshot.is_expanded = is_expanded;
        }

        std::size_t type(Luau::TypeId value, std::size_t depth = 0) {
            budget.check();

            if (!value) {
                return omitted_type();
            }

            if (const auto found = types.find(value); found != types.end()) {
                return found->second;
            }

            if (!reserve_node(depth)) {
                return omitted_type();
            }

            const auto index = snapshot.types.size();
            NativeType placeholder{};
            placeholder.kind = NativeTypeKind::Omitted;
            snapshot.types.push_back(std::move(placeholder));
            types.emplace(value, index);
            work.push_back({value, nullptr, index, depth, false});

            return index;
        }

        std::size_t pack(Luau::TypePackId value, std::size_t depth = 0) {
            budget.check();

            if (!value) {
                return omitted_pack();
            }

            if (const auto found = packs.find(value); found != packs.end()) {
                return found->second;
            }

            if (!reserve_node(depth)) {
                return omitted_pack();
            }

            const auto index = snapshot.packs.size();
            NativePack placeholder{};
            placeholder.kind = NativePackKind::Omitted;
            snapshot.packs.push_back(std::move(placeholder));
            packs.emplace(value, index);
            work.push_back({nullptr, value, index, depth, false});

            return index;
        }

        std::size_t named(
            Luau::TypeId value, const std::string &name, const std::string &qualifier,
            const std::vector<Luau::TypeId> &type_arguments, const std::vector<Luau::TypePackId> &pack_arguments
        ) {
            budget.check();

            if (!value ||
                !reserve_payload(type_arguments.size() + pack_arguments.size() + 1, name.size() + qualifier.size()) ||
                !reserve_node(0)) {
                return omitted_type();
            }

            const auto root = snapshot.types.size();
            NativeType result{};
            result.kind = NativeTypeKind::Named;
            result.state = static_cast<std::uint8_t>(NativeProvenance::Alias);
            result.name = name;
            result.qualifier = {!qualifier.empty(), qualifier};
            snapshot.types.push_back(NativeType{});

            type_list(result.types, type_arguments, 1);
            pack_list(result.packs, pack_arguments, 1);
            types.emplace(value, root);

            result.expansion = structure(value, 1);
            snapshot.types[root] = std::move(result);

            return root;
        }

        NativeSnapshot finish(std::size_t root, bool is_root_pack) {
            snapshot.root = root;
            snapshot.is_root_pack = is_root_pack;

            for (std::size_t cursor = 0; cursor < work.size(); ++cursor) {
                budget.check();
                const Work current = work[cursor];

                if (current.type) {
                    auto result =
                        Luau::visit([&](const auto &value) { return export_type(value, current); }, current.type->ty);

                    snapshot.types[current.index] = std::move(result);
                } else {
                    auto result =
                        Luau::visit([&](const auto &value) { return export_pack(value, current); }, current.pack->ty);

                    snapshot.packs[current.index] = std::move(result);
                }
            }

            budget.check();

            return std::move(snapshot);
        }

    private:
        bool reserve_node(std::size_t depth) {
            if (depth > limits.depth || nodes >= limits.nodes || budget.nodes >= 32768) {
                return false;
            }

            ++nodes;
            ++budget.nodes;

            return true;
        }

        NativeIndex structure(Luau::TypeId value, std::size_t depth) {
            budget.check();

            if (const auto found = structures.find(value); found != structures.end()) {
                return {true, found->second};
            }

            if (!reserve_node(depth)) {
                return {true, omitted_type()};
            }

            const auto index = snapshot.types.size();
            NativeType placeholder{};
            placeholder.kind = NativeTypeKind::Omitted;
            snapshot.types.push_back(std::move(placeholder));
            structures.emplace(value, index);
            work.push_back({value, nullptr, index, depth, true});

            return {true, index};
        }

        std::size_t forward(Luau::TypeId value, const Work &current) {
            if (!current.expand_structure) {
                return type(value, current.depth);
            }

            if (const auto found = types.find(current.type); found != types.end()) {
                types.emplace(value, found->second);
            }

            return structure(value, current.depth).index;
        }

        bool reserve_payload(std::size_t additional_edges, std::size_t additional_bytes) {
            budget.check();

            if (additional_edges > limits.edges - edges || additional_edges > 131072 - budget.edges) {
                return false;
            }

            additional_bytes += additional_edges * sizeof(std::size_t);

            if (additional_bytes > limits.bytes - bytes || additional_bytes > 2097152 - budget.bytes) {
                return false;
            }

            edges += additional_edges;
            bytes += additional_bytes;
            budget.edges += additional_edges;
            budget.bytes += additional_bytes;

            return true;
        }

        std::size_t omitted_type() {
            if (omitted_type_index == absent) {
                omitted_type_index = snapshot.types.size();
                NativeType value{};
                value.kind = NativeTypeKind::Omitted;
                snapshot.types.push_back(std::move(value));
            }

            return omitted_type_index;
        }

        std::size_t omitted_pack() {
            if (omitted_pack_index == absent) {
                omitted_pack_index = snapshot.packs.size();
                NativePack value{};
                value.kind = NativePackKind::Omitted;
                snapshot.packs.push_back(std::move(value));
            }

            return omitted_pack_index;
        }

        template <class Values>
        void type_list(rust::Vec<std::size_t> &output, const Values &values, std::size_t depth) {
            for (const auto value : values) {
                budget.check();
                output.push_back(type(value, depth));
            }
        }

        template <class Values>
        void pack_list(rust::Vec<std::size_t> &output, const Values &values, std::size_t depth) {
            for (const auto value : values) {
                budget.check();
                output.push_back(pack(value, depth));
            }
        }

        template <class Value> NativeType export_type(const Value &value, const Work &current) {
            NativeType result{};
            result.kind = NativeTypeKind::Omitted;
            const auto depth = current.depth + 1;

            if constexpr (std::is_same_v<Value, Luau::BoundType>) {
                if (!reserve_payload(1, 0)) {
                    return result;
                }

                result.kind = NativeTypeKind::Reference;
                result.types.push_back(forward(value.boundTo, current));
            } else if constexpr (std::is_same_v<Value, Luau::PrimitiveType>) {
                result.kind = NativeTypeKind::Primitive;
                result.state = static_cast<std::uint8_t>(NativePrimitiveState::Invalid);

                switch (value.type) {
                case Luau::PrimitiveType::NilType:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Nil);
                    break;

                case Luau::PrimitiveType::Boolean:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Boolean);
                    break;

                case Luau::PrimitiveType::Number:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Number);
                    break;

                case Luau::PrimitiveType::Integer:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Integer);
                    break;

                case Luau::PrimitiveType::String:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::String);
                    break;

                case Luau::PrimitiveType::Thread:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Thread);
                    break;

                case Luau::PrimitiveType::Function:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Function);
                    break;

                case Luau::PrimitiveType::Table:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Table);
                    break;

                case Luau::PrimitiveType::Buffer:
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Buffer);
                    break;
                }
            } else if constexpr (std::is_same_v<Value, Luau::AnyType> || std::is_same_v<Value, Luau::UnknownType> ||
                                 std::is_same_v<Value, Luau::NeverType>) {
                result.kind = NativeTypeKind::Primitive;

                if constexpr (std::is_same_v<Value, Luau::AnyType>) {
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Any);
                }

                if constexpr (std::is_same_v<Value, Luau::UnknownType>) {
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Unknown);
                }

                if constexpr (std::is_same_v<Value, Luau::NeverType>) {
                    result.state = static_cast<std::uint8_t>(NativePrimitiveState::Never);
                }
            } else if constexpr (std::is_same_v<Value, Luau::SingletonType>) {
                Luau::visit(
                    [&](const auto &singleton) {
                        using Singleton = std::decay_t<decltype(singleton)>;
                        if constexpr (std::is_same_v<Singleton, Luau::BooleanSingleton>) {
                            result.kind = NativeTypeKind::Singleton;
                            result.state = static_cast<std::uint8_t>(NativeSingletonState::Boolean);
                            result.boolean = singleton.value;
                        } else if constexpr (std::is_same_v<Singleton, Luau::StringSingleton>) {
                            if (!reserve_payload(0, singleton.value.size())) {
                                return;
                            }
                            result.kind = NativeTypeKind::Singleton;
                            result.state = static_cast<std::uint8_t>(NativeSingletonState::String);
                            for (unsigned char byte : singleton.value) {
                                budget.check();
                                result.bytes.push_back(byte);
                            }
                        } else {
                            static_assert(is_unsupported<Singleton>);
                        }
                    },
                    value.variant
                );
            } else if constexpr (std::is_same_v<Value, Luau::GenericType>) {
                if (!reserve_payload(0, value.explicitName ? value.name.size() : 0)) {
                    return result;
                }

                result.kind = NativeTypeKind::Generic;
                result.identity = current.index;
                result.has_explicit_name = value.explicitName;

                if (value.explicitName) {
                    result.name = value.name;
                }
            } else if constexpr (std::is_same_v<Value, Luau::FunctionType>) {
                std::size_t size = value.argNames.size() * sizeof(std::size_t);

                for (const auto &name : value.argNames) {
                    budget.check();

                    if (name) {
                        size += name->name.size();
                    }
                }

                if (!reserve_payload(value.generics.size() + value.genericPacks.size() + 2, size)) {
                    return result;
                }

                NativeFunction function{};
                type_list(function.generics, value.generics, depth);
                pack_list(function.generic_packs, value.genericPacks, depth);
                function.arguments = pack(value.argTypes, depth);
                function.returns = pack(value.retTypes, depth);
                function.is_checked = value.isCheckedFunction;

                for (const auto &name : value.argNames) {
                    budget.check();
                    function.argument_names.push_back({bool(name), name ? name->name : std::string{}});
                }

                result.kind = NativeTypeKind::Function;
                result.functions.push_back(std::move(function));
            } else if constexpr (std::is_same_v<Value, Luau::TableType>) {
                if (value.boundTo) {
                    if (!reserve_payload(1, 0)) {
                        return result;
                    }

                    result.kind = NativeTypeKind::Reference;
                    result.types.push_back(forward(*value.boundTo, current));

                    return result;
                }

                if (!current.expand_structure && value.name) {
                    const auto visibility = naming.table(value);
                    const auto &name = *value.name;
                    const bool expand = current.depth == 0;

                    if (!reserve_payload(
                            value.instantiatedTypeParams.size() + value.instantiatedTypePackParams.size() +
                                std::size_t(expand),
                            name.size() + visibility.qualifier.size()
                        )) {
                        return result;
                    }

                    result.kind = NativeTypeKind::Named;

                    result.state = static_cast<std::uint8_t>(
                        visibility.is_verified ? NativeProvenance::Alias : NativeProvenance::Unverified
                    );

                    result.name = name;
                    result.qualifier = {!visibility.qualifier.empty(), visibility.qualifier};
                    type_list(result.types, value.instantiatedTypeParams, depth);
                    pack_list(result.packs, value.instantiatedTypePackParams, depth);

                    if (expand) {
                        result.expansion = structure(current.type, depth);
                    }

                    return result;
                }

                std::size_t size = 0;
                std::size_t count = value.indexer ? 2 : 0;

                for (const auto &[name, property] : value.props) {
                    budget.check();
                    size += name.size();
                    count += std::size_t(bool(property.readTy)) + std::size_t(bool(property.writeTy));
                }

                if (!reserve_payload(count, size)) {
                    return result;
                }

                NativeTable table{};
                table.state = NativeTableState::Invalid;

                switch (value.state) {
                case Luau::TableState::Sealed:
                    table.state = NativeTableState::Sealed;
                    break;

                case Luau::TableState::Unsealed:
                    table.state = NativeTableState::Unsealed;
                    break;

                case Luau::TableState::Free:
                    table.state = NativeTableState::Free;
                    break;

                case Luau::TableState::Generic:
                    table.state = NativeTableState::Generic;
                    break;

                case Luau::TableState::Exact:
                    table.state = NativeTableState::Exact;
                    break;
                }

                for (const auto &[name, property] : value.props) {
                    budget.check();
                    NativeProperty output{};

                    for (unsigned char byte : name) {
                        budget.check();
                        output.name.push_back(byte);
                    }

                    if (property.readTy) {
                        output.read = {true, type(*property.readTy, depth)};
                    }

                    if (property.writeTy) {
                        output.write = {true, type(*property.writeTy, depth)};
                    }

                    output.is_shared = property.isShared();
                    table.properties.push_back(std::move(output));
                }

                if (value.indexer) {
                    table.indexers.push_back(
                        {type(value.indexer->indexType, depth),
                            type(value.indexer->indexResultType, depth),
                            value.indexer->isReadOnly}
                    );
                }

                result.kind = NativeTypeKind::Table;
                result.tables.push_back(std::move(table));
            } else if constexpr (std::is_same_v<Value, Luau::ExternType>) {
                if (!reserve_payload(0, value.name.size())) {
                    return result;
                }

                result.kind = NativeTypeKind::Named;
                result.state = static_cast<std::uint8_t>(NativeProvenance::External);
                const auto separator = value.name.find('.');

                if (separator != std::string::npos && value.name.compare(0, 7, "typeof(") != 0) {
                    result.name = value.name.substr(separator + 1);
                    result.qualifier = {true, value.name.substr(0, separator)};
                } else {
                    result.name = value.name;
                }
            } else if constexpr (std::is_same_v<Value, Luau::UnionType>) {
                const auto members = reduce_union(value, budget, builtins, limits);

                if (!reserve_payload(members.size(), 0)) {
                    return result;
                }

                result.kind = members.size() == 1 ? NativeTypeKind::Reference : NativeTypeKind::Union;

                for (const auto member : members) {
                    budget.check();

                    if (has_ambiguous_name(member, members) && reserve_node(depth) && reserve_payload(2, 0)) {
                        NativeType intersection{};
                        intersection.kind = NativeTypeKind::Intersection;
                        intersection.types.push_back(type(member, depth));
                        intersection.types.push_back(structure(member, depth).index);
                        result.types.push_back(snapshot.types.size());
                        snapshot.types.push_back(std::move(intersection));
                    } else {
                        result.types.push_back(type(member, depth));
                    }
                }
            } else if constexpr (std::is_same_v<Value, Luau::IntersectionType>) {
                if (!reserve_payload(value.parts.size(), 0)) {
                    return result;
                }

                result.kind = NativeTypeKind::Intersection;
                type_list(result.types, value.parts, depth);
            } else if constexpr (std::is_same_v<Value, Luau::MetatableType>) {
                const auto name = naming.metatable(value, current.type, current.expand_structure);

                if (name.is_verified && !current.expand_structure) {
                    const bool expand = current.depth == 0;

                    if (!reserve_payload(
                            std::size_t(expand) + name.type_arguments.size() + name.pack_arguments.size(),
                            name.name.size() + name.qualifier.size()
                        )) {
                        return result;
                    }

                    result.kind = NativeTypeKind::Named;
                    result.state = static_cast<std::uint8_t>(NativeProvenance::Alias);
                    result.name = name.name;
                    result.qualifier = {!name.qualifier.empty(), name.qualifier};
                    type_list(result.types, name.type_arguments, depth);
                    pack_list(result.packs, name.pack_arguments, depth);

                    if (expand) {
                        result.expansion = structure(current.type, depth);
                    }

                    return result;
                }

                if (!reserve_payload(2, 0)) {
                    return result;
                }

                result.kind = NativeTypeKind::Metatable;
                result.types.push_back(type(value.table, depth));
                result.types.push_back(type(value.metatable, depth));
            } else if constexpr (std::is_same_v<Value, Luau::NegationType>) {
                if (!reserve_payload(1, 0)) {
                    return result;
                }

                result.kind = NativeTypeKind::Negation;
                result.types.push_back(type(value.ty, depth));
            } else if constexpr (std::is_same_v<Value, Luau::PendingExpansionType> ||
                                 std::is_same_v<Value, Luau::TypeFunctionInstanceType>) {
                std::string_view name;
                std::string_view prefix;

                if constexpr (std::is_same_v<Value, Luau::PendingExpansionType>) {
                    name = value.name.value;

                    if (value.prefix) {
                        prefix = value.prefix->value;
                    }
                } else if (value.userFuncName) {
                    name = value.userFuncName->value;
                } else {
                    name = value.function->name;
                }

                if (!reserve_payload(
                        value.typeArguments.size() + value.packArguments.size(),
                        name.size() + prefix.size()
                    )) {
                    return result;
                }

                NativeApplication application{};
                application.name = std::string(name);
                application.qualifier = {!prefix.empty(), std::string(prefix)};

                application.state = std::is_same_v<Value, Luau::PendingExpansionType>
                                        ? NativeApplicationState::Pending
                                        : NativeApplicationState::Unreduced;

                type_list(application.type_arguments, value.typeArguments, depth);
                pack_list(application.pack_arguments, value.packArguments, depth);
                result.kind = NativeTypeKind::Application;
                result.applications.push_back(std::move(application));
            } else if constexpr (std::is_same_v<Value, Luau::LazyType>) {
                if (const auto unwrapped = value.unwrapped.load()) {
                    if (!reserve_payload(1, 0)) {
                        return result;
                    }

                    result.kind = NativeTypeKind::Reference;
                    result.types.push_back(forward(unwrapped, current));
                } else {
                    result.kind = NativeTypeKind::Unresolved;
                    result.state = static_cast<std::uint8_t>(NativeUnresolvedState::Lazy);
                }
            } else if constexpr (std::is_same_v<Value, Luau::FreeType> || std::is_same_v<Value, Luau::BlockedType> ||
                                 std::is_same_v<Value, Luau::ErrorType> || std::is_same_v<Value, Luau::NoRefineType>) {
                result.kind = NativeTypeKind::Unresolved;
                result.state = static_cast<std::uint8_t>(NativeUnresolvedState::Free);

                if constexpr (std::is_same_v<Value, Luau::BlockedType>) {
                    result.state = static_cast<std::uint8_t>(NativeUnresolvedState::Blocked);
                }

                if constexpr (std::is_same_v<Value, Luau::ErrorType>) {
                    result.state = static_cast<std::uint8_t>(NativeUnresolvedState::Error);
                }

                if constexpr (std::is_same_v<Value, Luau::NoRefineType>) {
                    result.state = static_cast<std::uint8_t>(NativeUnresolvedState::InternalRefinement);
                }
            } else {
                static_assert(is_unsupported<Value>);
            }

            return result;
        }

        template <class Value> NativePack export_pack(const Value &value, const Work &current) {
            NativePack result{};
            result.kind = NativePackKind::Omitted;
            const auto depth = current.depth + 1;

            if constexpr (std::is_same_v<Value, Luau::BoundTypePack>) {
                if (!reserve_payload(1, 0)) {
                    return result;
                }

                result.kind = NativePackKind::Reference;
                result.packs.push_back(pack(value.boundTo, depth));
            } else if constexpr (std::is_same_v<Value, Luau::TypePack>) {
                if (!reserve_payload(value.head.size() + std::size_t(bool(value.tail)), 0)) {
                    return result;
                }

                result.kind = NativePackKind::Sequence;
                type_list(result.types, value.head, depth);

                if (value.tail) {
                    result.tail = {true, pack(*value.tail, depth)};
                }
            } else if constexpr (std::is_same_v<Value, Luau::VariadicTypePack>) {
                if (!reserve_payload(1, 0)) {
                    return result;
                }

                result.kind = NativePackKind::Variadic;
                result.types.push_back(type(value.ty, depth));
                result.is_hidden = value.hidden;
            } else if constexpr (std::is_same_v<Value, Luau::GenericTypePack>) {
                if (!reserve_payload(0, value.explicitName ? value.name.size() : 0)) {
                    return result;
                }

                result.kind = NativePackKind::Generic;
                result.identity = current.index;
                result.has_explicit_name = value.explicitName;

                if (value.explicitName) {
                    result.name = value.name;
                }
            } else if constexpr (std::is_same_v<Value, Luau::TypeFunctionInstanceTypePack>) {
                if (!reserve_payload(
                        value.typeArguments.size() + value.packArguments.size(),
                        value.function->name.size()
                    )) {
                    return result;
                }

                NativeApplication application{};
                application.name = value.function->name;
                application.state = NativeApplicationState::Unreduced;
                type_list(application.type_arguments, value.typeArguments, depth);
                pack_list(application.pack_arguments, value.packArguments, depth);
                result.kind = NativePackKind::Application;
                result.applications.push_back(std::move(application));
            } else if constexpr (std::is_same_v<Value, Luau::FreeTypePack> ||
                                 std::is_same_v<Value, Luau::BlockedTypePack> ||
                                 std::is_same_v<Value, Luau::ErrorTypePack>) {
                result.kind = NativePackKind::Unresolved;
                result.state = static_cast<std::uint8_t>(NativeUnresolvedState::Free);

                if constexpr (std::is_same_v<Value, Luau::BlockedTypePack>) {
                    result.state = static_cast<std::uint8_t>(NativeUnresolvedState::Blocked);
                }

                if constexpr (std::is_same_v<Value, Luau::ErrorTypePack>) {
                    result.state = static_cast<std::uint8_t>(NativeUnresolvedState::Error);
                }
            } else {
                static_assert(is_unsupported<Value>);
            }

            return result;
        }

        PresentationBudget &budget;
        Luau::BuiltinTypes &builtins;
        const PresentationLimits limits;
        Naming naming;
        std::size_t nodes = 0;
        std::size_t edges = 0;
        std::size_t bytes = 0;
        std::size_t omitted_type_index = absent;
        std::size_t omitted_pack_index = absent;
        NativeSnapshot snapshot{};
        std::unordered_map<Luau::TypeId, std::size_t> types;
        std::unordered_map<Luau::TypeId, std::size_t> structures;
        std::unordered_map<Luau::TypePackId, std::size_t> packs;
        std::vector<Work> work;
    };

    Extraction::Extraction(
        PresentationBudget &budget, bool is_expanded, Luau::ScopePtr scope, const Luau::Module &module,
        const Luau::SourceModule &source, Luau::BuiltinTypes &builtins, const Luau::ModuleResolver &modules
    )
        : traversal(
              std::make_unique<Traversal>(budget, is_expanded, std::move(scope), module, source, builtins, modules)
          ) {}

    Extraction::~Extraction() = default;

    std::size_t Extraction::type(Luau::TypeId value) {
        return traversal->type(value);
    }

    std::size_t Extraction::pack(Luau::TypePackId value) {
        return traversal->pack(value);
    }

    std::size_t Extraction::named(
        Luau::TypeId value, const std::string &name, const std::string &qualifier,
        const std::vector<Luau::TypeId> &type_arguments, const std::vector<Luau::TypePackId> &pack_arguments
    ) {
        return traversal->named(value, name, qualifier, type_arguments, pack_arguments);
    }

    NativeSnapshot Extraction::finish(std::size_t root, bool is_root_pack) {
        return traversal->finish(root, is_root_pack);
    }
}
