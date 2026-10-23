#include "enumerations.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Lexer.h"
#include "Luau/Scope.h"
#include "Luau/StringUtils.h"
#include "Luau/Type.h"
#include "Luau/TypeArena.h"
#include "instar-bridge/src/bridge.rs.h"

#include <optional>
#include <stdexcept>
#include <string>
#include <unordered_set>
#include <utility>

namespace instar::luau {
    namespace {
        void validate_name(const std::string &name) {
            if (name.empty() || (name.front() >= '0' && name.front() <= '9') || !Luau::isIdentifier(name) ||
                Luau::Lexer::isReserved(name)) {
                throw std::invalid_argument("Invalid Roblox enumeration name `" + name + "`.");
            }
        }

        void validate(Luau::GlobalTypes &globals, rust::Slice<const Enumeration> enumerations) {
            const auto &scope = globals.globalScope;

            for (const char *name : {"Enum", "EnumItem", "Enumerations"}) {
                if (scope->lookupType(name)) {
                    throw std::invalid_argument(
                        "Roblox enumeration type binding `" + std::string(name) + "` already exists."
                    );
                }
            }

            if (scope->linearSearchForBinding("Enum")) {
                throw std::invalid_argument("Roblox enumeration global binding `Enum` already exists.");
            }

            for (auto current = scope; current; current = current->parent) {
                if (current->importedTypeBindings.find("Enum") != current->importedTypeBindings.end() ||
                    current->importedModules.find("Enum") != current->importedModules.end()) {
                    throw std::invalid_argument("Roblox enumeration type namespace `Enum` already exists.");
                }
            }

            std::unordered_set<std::string> names;

            for (const Enumeration &enumeration : enumerations) {
                const std::string name(enumeration.name);
                validate_name(name);

                if (!names.insert(name).second) {
                    throw std::invalid_argument("Duplicate Roblox enumeration `" + name + "`.");
                }

                if (name == "GetEnums" || scope->lookupType("Enum." + name)) {
                    throw std::invalid_argument("Conflicting Roblox enumeration `" + name + "`.");
                }

                std::unordered_set<std::string> items;

                for (const rust::String &value : enumeration.items) {
                    const std::string item(value);

                    if (item.empty() || item.find('\0') != std::string::npos) {
                        throw std::invalid_argument(
                            "Roblox enumeration `" + name +
                            "` contains an empty item name or an item name with a null character."
                        );
                    }

                    if (!items.insert(item).second) {
                        throw std::invalid_argument(
                            std::string("Duplicate Roblox enumeration item `")
                                .append(name)
                                .append(".")
                                .append(item)
                                .append("`.")
                        );
                    }

                    if (item == "GetEnumItems" || item == "FromName" || item == "FromValue") {
                        throw std::invalid_argument(
                            std::string("Conflicting Roblox enumeration item `")
                                .append(name)
                                .append(".")
                                .append(item)
                                .append("`.")
                        );
                    }
                }
            }
        }

        Luau::TypeId external(
            Luau::GlobalTypes &globals, const std::string &name, std::optional<Luau::TypeId> parent,
            std::optional<Luau::TypeId> metatable, const std::string &documentation
        ) {
            const auto type = globals.globalTypes.addType(
                Luau::ExternType{name, {}, parent, metatable, {}, {}, "@roblox", std::nullopt}
            );

            Luau::asMutable(type)->documentationSymbol = documentation;

            return type;
        }

        Luau::TypeId array(Luau::GlobalTypes &globals, Luau::TypeId item) {
            return globals.globalTypes.addType(
                Luau::TableType{{},
                    Luau::TableIndexer{globals.builtinTypes->numberType, item},
                    {},
                    Luau::TableState::Sealed}
            );
        }
    }

    void construct_enumerations(Luau::GlobalTypes &globals, rust::Slice<const Enumeration> enumerations) {
        validate(globals, enumerations);

        auto &arena = globals.globalTypes;
        const auto builtins = globals.builtinTypes;
        const auto enum_metatable = arena.addType(Luau::TableType{Luau::TableState::Sealed, {}});
        const auto item_metatable = arena.addType(Luau::TableType{Luau::TableState::Sealed, {}});
        const auto base = external(globals, "Enum", builtins->externType, enum_metatable, "@roblox/globaltype/Enum");

        const auto item =
            external(globals, "EnumItem", builtins->externType, item_metatable, "@roblox/globaltype/EnumItem");

        const auto root_metatable = arena.addType(Luau::TableType{Luau::TableState::Sealed, {}});

        const auto root =
            external(globals, "Enumerations", builtins->externType, root_metatable, "@roblox/global/Enum");

        Luau::attachTag(root, "instar.namespace");

        auto &base_properties = Luau::getMutable<Luau::ExternType>(base)->props;

        base_properties.emplace(
            "GetEnumItems",
            Luau::Property::rw(Luau::makeFunction(arena, base, {}, {array(globals, builtins->anyType)}))
        );

        base_properties.emplace(
            "FromValue",
            Luau::Property::rw(Luau::makeFunction(arena, base, {builtins->numberType}, {"Number"}, {builtins->anyType}))
        );

        base_properties.emplace(
            "FromName",
            Luau::Property::rw(Luau::makeFunction(arena, base, {builtins->stringType}, {"Name"}, {builtins->anyType}))
        );

        Luau::assignPropDocumentationSymbols(base_properties, "@roblox/globaltype/Enum");

        auto &item_properties = Luau::getMutable<Luau::ExternType>(item)->props;
        item_properties.emplace("Name", Luau::Property::rw(builtins->stringType));
        item_properties.emplace("Value", Luau::Property::rw(builtins->numberType));
        item_properties.emplace("EnumType", Luau::Property::rw(base));

        item_properties.emplace(
            "IsA",
            Luau::Property::rw(
                Luau::makeFunction(arena, item, {builtins->stringType}, {"enumName"}, {builtins->booleanType})
            )
        );

        Luau::assignPropDocumentationSymbols(item_properties, "@roblox/globaltype/EnumItem");

        auto *root_type = Luau::getMutable<Luau::ExternType>(root);
        root_type->indexer = Luau::TableIndexer{builtins->stringType, base};

        root_type->props.emplace(
            "GetEnums",
            Luau::Property::rw(Luau::makeFunction(arena, root, {}, {array(globals, base)}))
        );

        Luau::assignPropDocumentationSymbols(root_type->props, "@roblox/globaltype/Enums");

        auto &bindings = globals.globalScope->importedTypeBindings["Enum"];

        for (const Enumeration &enumeration : enumerations) {
            const std::string name(enumeration.name);
            const std::string documentation = "@roblox/global/Enum." + name;
            const auto member = external(globals, "Enum." + name, item, item_metatable, documentation);
            const auto collection = external(globals, "typeof(Enum." + name + ")", base, enum_metatable, documentation);
            Luau::attachTag(collection, "instar.namespace");
            auto &properties = Luau::getMutable<Luau::ExternType>(collection)->props;

            for (const rust::String &value : enumeration.items) {
                auto property = Luau::Property::rw(member);
                property.documentationSymbol = "@roblox/enum/" + name + "." + std::string(value);
                properties.emplace(std::string(value), std::move(property));
            }

            const auto optional = Luau::makeOption(builtins, arena, member);

            properties.emplace(
                "GetEnumItems",
                Luau::Property::rw(Luau::makeFunction(arena, collection, {}, {array(globals, member)}))
            );

            properties.emplace(
                "FromName",
                Luau::Property::rw(Luau::makeFunction(arena, collection, {builtins->stringType}, {"name"}, {optional}))
            );

            properties.emplace(
                "FromValue",
                Luau::Property::rw(Luau::makeFunction(arena, collection, {builtins->numberType}, {"value"}, {optional}))
            );

            for (const char *method : {"GetEnumItems", "FromName", "FromValue"}) {
                properties.at(method).documentationSymbol = base_properties.at(method).documentationSymbol;
            }

            auto property = Luau::Property::rw(collection);
            property.documentationSymbol = documentation;
            root_type->props.emplace(name, std::move(property));
            bindings.emplace(name, Luau::TypeFun{member});
        }

        globals.globalScope->exportedTypeBindings.emplace("Enum", Luau::TypeFun{base});
        globals.globalScope->exportedTypeBindings.emplace("EnumItem", Luau::TypeFun{item});
        globals.globalScope->exportedTypeBindings.emplace("Enumerations", Luau::TypeFun{root});
        Luau::addGlobalBinding(globals, "Enum", root, "@roblox");
        Luau::persist(base);
        Luau::persist(item);
        Luau::persist(root);
    }
}
