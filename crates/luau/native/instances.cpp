#include "instances.hpp"

#include "Luau/BuiltinDefinitions.h"
#include "Luau/Type.h"
#include "Luau/TypePack.h"
#include "Luau/TypeUtils.h"
#include "instar-bridge/src/bridge.rs.h"

#include <memory>
#include <stdexcept>
#include <string>
#include <unordered_map>
#include <utility>

namespace instar::luau {
    namespace {
        struct Metadata final : Luau::ClassUserData {
            std::string name;
            std::string class_name;
            std::string identity;
            std::optional<Luau::TypeId> parent;
            std::vector<Luau::TypeId> children;
        };

        const Metadata *metadata(Luau::TypeId type) {
            const auto *external = Luau::get<Luau::ExternType>(Luau::follow(type));

            return external ? dynamic_cast<const Metadata *>(external->userData.get()) : nullptr;
        }

        Luau::TypeId superclass(const Luau::ExternType &type) {
            if (!type.parent) {
                throw std::logic_error("instance type has no superclass");
            }

            return *type.parent;
        }

        bool has_property(Luau::TypeId type, const std::string &name) {
            return Luau::lookupExternTypeProp(Luau::get<Luau::ExternType>(Luau::follow(type)), name) ||
                   Luau::hasTag(type, "instar.property." + name);
        }

        void specialize_wait(
            Luau::TypeArena &arena, Luau::NotNull<Luau::BuiltinTypes> builtins, const Luau::ExternType *instance,
            Luau::TypeId type
        ) {
            auto *external = Luau::getMutable<Luau::ExternType>(type);
            const auto *property = Luau::lookupExternTypeProp(external, "WaitForChild");

            if (!property || !property->readTy || metadata(type)->children.empty()) {
                return;
            }

            const auto original = Luau::follow(*property->readTy);
            const auto *intersection = Luau::get<Luau::IntersectionType>(original);
            const auto parts = intersection ? intersection->parts : std::vector<Luau::TypeId>{original};
            std::vector<Luau::TypeId> overloads;
            std::unordered_map<std::string, Luau::TypeId> children;

            for (Luau::TypeId child : metadata(type)->children) {
                auto [found, inserted] = children.emplace(metadata(child)->name, child);

                if (!inserted) {
                    found->second = nullptr;
                }
            }

            for (const auto &[name, child] : children) {
                if (!child) {
                    continue;
                }

                for (Luau::TypeId part : parts) {
                    const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(part));

                    if (!function) {
                        continue;
                    }

                    auto [arguments, tail] = Luau::flatten(function->argTypes);
                    const auto [results, result_tail] = Luau::flatten(function->retTypes);

                    if (arguments.size() < 2 || results.size() != 1 || result_tail ||
                        Luau::follow(arguments[1]) != builtins->stringType ||
                        Luau::get<Luau::ExternType>(Luau::stripNil(builtins, arena, results[0])) != instance) {
                        continue;
                    }

                    auto specialized = *function;
                    arguments[0] = type;
                    arguments[1] = arena.addType(Luau::SingletonType{Luau::StringSingleton{name}});
                    specialized.argTypes = arena.addTypePack(std::move(arguments), tail);
                    const auto result = Luau::isOptional(results[0]) ? Luau::makeOption(builtins, arena, child) : child;
                    specialized.retTypes = arena.addTypePack({result});
                    overloads.push_back(arena.addType(std::move(specialized)));
                }
            }

            if (!overloads.empty()) {
                overloads.insert(overloads.end(), parts.begin(), parts.end());
                auto specialized = *property;
                specialized.readTy = arena.addType(Luau::IntersectionType{std::move(overloads)});
                external->props.insert_or_assign("WaitForChild", std::move(specialized));
            }
        }

        void
        children(const Metadata &instance, std::string_view name, bool recursive, std::vector<Luau::TypeId> &found) {
            for (Luau::TypeId child : instance.children) {
                if (metadata(child)->name == name) {
                    found.push_back(child);
                }
            }

            if (found.empty() && recursive) {
                for (Luau::TypeId child : instance.children) {
                    std::vector<Luau::TypeId> nested;
                    children(*metadata(child), name, true, nested);
                    found.insert(found.end(), nested.begin(), nested.end());
                }
            }
        }
    }

    Instances::Instances(
        const std::string &hierarchy, const Luau::ScopePtr &scope, rust::Slice<const Instance> instances,
        Luau::NotNull<Luau::BuiltinTypes> builtins
    ) {
        const auto instance = scope->lookupType("Instance");
        const auto *base_instance = instance ? Luau::get<Luau::ExternType>(Luau::follow(instance->type)) : nullptr;
        types.reserve(instances.size());

        for (size_t index = 0; index < instances.size(); ++index) {
            const auto &node = instances[index];

            if ((index == 0 && node.has_parent) || (index != 0 && (!node.has_parent || node.parent >= index))) {
                throw std::invalid_argument("instance hierarchy must contain one root with parents before children");
            }

            const auto declared = scope->lookupType(std::string(node.class_name));
            const auto *base = declared ? Luau::get<Luau::ExternType>(Luau::follow(declared->type)) : nullptr;

            if (!declared || !base || !base_instance || !Luau::isSubclass(base, base_instance)) {
                throw std::invalid_argument("unknown instance class `" + std::string(node.class_name) + "`");
            }

            auto data = std::make_shared<Metadata>();
            data->name = std::string(node.name);
            data->class_name = std::string(node.class_name);

            data->identity = instance_identity(hierarchy, index);

            if (node.has_parent) {
                data->parent = types[node.parent];
            }

            auto type = arena.addType(
                Luau::ExternType{base->name,
                    {},
                    declared->type,
                    base->metatable,
                    {},
                    data,
                    base->definitionModuleName,
                    base->definitionLocation}
            );

            auto *external = Luau::getMutable<Luau::ExternType>(type);

            if (const auto *parent = Luau::lookupExternTypeProp(base, "Parent"); parent && parent->readTy) {
                external->props.emplace(
                    "Parent",
                    Luau::Property::create(data->parent.value_or(builtins->nilType), parent->writeTy)
                );
            }

            if (node.has_parent) {
                auto *parent = Luau::getMutable<Luau::ExternType>(types[node.parent]);
                static_cast<Metadata *>(parent->userData.get())->children.push_back(type);
                const auto parent_base = superclass(*parent);
                const std::string name(node.name);

                if (!has_property(parent_base, name)) {
                    auto [property, inserted] = parent->props.emplace(name, Luau::Property::readonly(type));

                    if (!inserted) {
                        const auto readable = property->second.readTy;

                        if (!readable) {
                            throw std::logic_error("instance child property is not readable");
                        }

                        property->second = Luau::Property::readonly(arena.addType(Luau::UnionType{{*readable, type}}));
                    }
                }
            }

            identities.emplace(data->identity, type);

            types.push_back(type);
        }

        for (Luau::TypeId type : types) {
            specialize_wait(arena, builtins, base_instance, type);

            if (metadata(type)->class_name == "DataModel") {
                auto *external = Luau::getMutable<Luau::ExternType>(type);

                for (const std::string name : {"Workspace", "workspace", "lighting", "RunService"}) {
                    const auto *property = Luau::lookupExternTypeProp(external, name);

                    const auto service = lookup_instance(
                        type,
                        InstanceQuery::Service,
                        name == "workspace"  ? "Workspace"
                        : name == "lighting" ? "Lighting"
                                             : name
                    );

                    if (property && property->readTy && service) {
                        auto specialized = *property;
                        specialized.readTy = *service;
                        external->props.insert_or_assign(name, std::move(specialized));
                    }
                }
            }
        }

        for (Luau::TypeId type : types) {
            Luau::persist(type);
        }

        Luau::freeze(arena);
    }

    void Instances::bind(Luau::GlobalTypes &globals, const Luau::ScopePtr &scope, std::optional<size_t> script) const {
        if (script) {
            if (*script >= types.size()) {
                throw std::invalid_argument("script index is outside its instance hierarchy");
            }

            Luau::addGlobalBinding(globals, scope, "script", types[*script], "@roblox");
        }

        if (types.empty()) {
            return;
        }

        const auto *root = metadata(types[0]);

        if (root->class_name == "DataModel") {
            Luau::addGlobalBinding(globals, scope, "game", types[0], "@roblox");

            if (auto workspace = lookup_instance(types[0], InstanceQuery::Service, "Workspace")) {
                Luau::addGlobalBinding(globals, scope, "workspace", *workspace, "@roblox");
            }
        } else if (root->class_name == "Workspace") {
            Luau::addGlobalBinding(globals, scope, "workspace", types[0], "@roblox");
        }
    }

    std::optional<std::string> Instances::global(InstanceQuery query, std::optional<size_t> script) const {
        if (query == InstanceQuery::Script) {
            return script && *script < types.size() ? std::optional<std::string>{metadata(types[*script])->identity}
                                                    : std::nullopt;
        }

        if (types.empty()) {
            return std::nullopt;
        }

        const auto *root = metadata(types[0]);

        if (query == InstanceQuery::Game && root->class_name == "DataModel") {
            return root->identity;
        }

        if (query == InstanceQuery::Workspace) {
            if (root->class_name == "Workspace") {
                return root->identity;
            }

            if (root->class_name == "DataModel") {
                if (const auto workspace = lookup_instance(types[0], InstanceQuery::Service, "Workspace")) {
                    return metadata(*workspace)->identity;
                }
            }
        }

        return std::nullopt;
    }

    std::optional<std::string>
    Instances::resolve(const std::string &context, InstanceQuery query, const std::string &name) const {
        const auto found = identities.find(context);

        if (found == identities.end()) {
            return std::nullopt;
        }

        if (query == InstanceQuery::Property) {
            const auto *external = Luau::get<Luau::ExternType>(found->second);

            if (has_property(superclass(*external), name)) {
                const auto property = external->props.find(name);
                const auto readable = property != external->props.end() ? property->second.readTy : std::nullopt;

                if (readable) {
                    if (const auto *value = metadata(*readable)) {
                        return value->identity;
                    }
                }

                return std::nullopt;
            }

            query = InstanceQuery::Child;
        }

        const auto result = lookup_instance(found->second, query, name);

        return result ? std::optional<std::string>{metadata(*result)->identity} : std::nullopt;
    }

    std::string instance_identity(const std::string &hierarchy, size_t script) {
        return "instar://" + hierarchy + "/" + std::to_string(script);
    }

    std::optional<Luau::TypeId> lookup_instance(Luau::TypeId type, InstanceQuery query, std::string_view name) {
        const auto *instance = metadata(type);

        if (!instance) {
            return std::nullopt;
        }

        std::vector<Luau::TypeId> found;

        switch (query) {
        case InstanceQuery::Parent:
            return instance->parent;

        case InstanceQuery::Child:

        case InstanceQuery::Descendant:
            children(*instance, name, query == InstanceQuery::Descendant, found);
            break;

        case InstanceQuery::Service:
            for (Luau::TypeId child : instance->children) {
                if (metadata(child)->class_name == name) {
                    found.push_back(child);
                }
            }

            break;

        case InstanceQuery::Ancestor:
            while (instance->parent) {
                const auto parent = *instance->parent;
                instance = metadata(parent);

                if (instance->name == name) {
                    return parent;
                }
            }

            break;

        default:
            break;
        }

        return found.size() == 1 ? std::optional<Luau::TypeId>{found[0]} : std::nullopt;
    }
}
