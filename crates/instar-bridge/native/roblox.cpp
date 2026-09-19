#include "roblox.hpp"

#include "Luau/Ast.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/ConstraintSolver.h"
#include "Luau/Error.h"
#include "Luau/LValue.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "Luau/TypeInfer.h"
#include "Luau/TypePack.h"
#include "Luau/TypeUtils.h"

#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <vector>

LUAU_FASTFLAG(LuauCyclicRequireTypeInference)

namespace instar {
    namespace {
        enum class MagicKind {
            Constructor,
            Service,
            IsA,
            ChildClass,
            ChildType,
            AncestorClass,
            AncestorType,
        };

        struct ClassInfo {
            bool service;
            bool creatable;
        };

        struct NodeInfo {
            std::string name;
            std::string class_name;
        };

        struct Metadata {
            std::unordered_map<std::string, ClassInfo> classes;
            std::vector<NodeInfo> nodes;
        };

        std::optional<std::string> argument(const Luau::AstExprCall &call) {
            if (call.args.size == 0) {
                return std::nullopt;
            }

            const auto *value = call.args.data[0]->as<Luau::AstExprConstantString>();

            return value ? std::optional<std::string>(std::string(value->value.data, value->value.size)) : std::nullopt;
        }

        std::optional<Luau::TypeId> class_type(const Luau::Scope &scope, const Metadata &metadata, std::string_view name, MagicKind kind) {
            const auto found = metadata.classes.find(std::string(name));

            if (found == metadata.classes.end() || (kind == MagicKind::Constructor && !found->second.creatable) ||
                (kind == MagicKind::Service && !found->second.service)) {
                return std::nullopt;
            }

            const std::optional<Luau::TypeFun> type = scope.lookupType(std::string(name));

            return type ? std::optional<Luau::TypeId>(type->type) : std::nullopt;
        }

        std::optional<Luau::TypeId> class_type(const Luau::Scope &scope, const Metadata &metadata, std::string_view name) {
            if (metadata.classes.find(std::string(name)) == metadata.classes.end()) {
                return std::nullopt;
            }

            const std::optional<Luau::TypeFun> type = scope.lookupType(std::string(name));

            return type ? std::optional<Luau::TypeId>(type->type) : std::nullopt;
        }

        std::string invalid_message(MagicKind kind, std::string_view name) {
            switch (kind) {
            case MagicKind::Constructor:
                return "Instance.new does not support class '" + std::string(name) + "'";

            case MagicKind::Service:
                return "GetService does not support service '" + std::string(name) + "'";

            case MagicKind::IsA:
                return "IsA does not support class '" + std::string(name) + "'";

            default:
                return "instance lookup does not support class '" + std::string(name) + "'";
            }
        }

        void report_old(Luau::TypeChecker &checker, const Luau::AstExpr &expression, std::string message) {
            checker.reportError(Luau::TypeError{expression.location, Luau::GenericError{std::move(message)}});
        }

        void report_new(const Luau::MagicFunctionCallContext &context, const Luau::AstExpr &expression, std::string message) {
            if (FFlag::LuauCyclicRequireTypeInference) {
                context.solver->reportError(Luau::GenericError{std::move(message)}, expression.location, *context.constraint->moduleName);
            } else {
                context.solver->DEPRECATED_reportError(Luau::TypeError{expression.location, Luau::GenericError{std::move(message)}});
            }
        }

        class MagicFunction final : public Luau::MagicFunction {
          public:
            MagicFunction(MagicKind kind, std::shared_ptr<const Metadata> metadata) : kind(kind), metadata(std::move(metadata)) {}

            std::optional<Luau::WithPredicate<Luau::TypePackId>> handleOldSolver(
                Luau::TypeChecker &checker, const Luau::ScopePtr &scope, const Luau::AstExprCall &call, Luau::WithPredicate<Luau::TypePackId> predicate
            ) override {

                if (kind == MagicKind::IsA) {
                    return old_is_a(checker, scope, call, std::move(predicate));
                }

                if (call.args.size == 0 || call.args.size > maximum_arguments()) {
                    return std::nullopt;
                }

                const std::optional<std::string> name = argument(call);
                const std::optional<Luau::TypeId> type = name ? class_type(*scope, *metadata, *name, kind) : std::nullopt;

                if (!type) {
                    if (name) {
                        report_old(checker, *call.args.data[0], invalid_message(kind, *name));
                    }

                    return std::nullopt;
                }

                const auto [parameters, tail] = Luau::flatten(predicate.type);
                const size_t offset = call.self ? 1 : 0;

                if (tail || parameters.size() < offset + call.args.size) {
                    return std::nullopt;
                }

                checker.unify(parameters[offset], checker.stringType, scope, call.args.data[0]->location);

                Luau::TypeId result = *type;

                if (returns_optional()) {
                    result = checker.currentModule->internalTypes->addType(Luau::UnionType{{checker.builtinTypes->nilType, result}});
                }

                return Luau::WithPredicate<Luau::TypePackId>{checker.currentModule->internalTypes->addTypePack({result})};
            }

            bool infer(const Luau::MagicFunctionCallContext &context) override {
                if (kind == MagicKind::IsA && context.callSite->args.size == 1) {
                    const std::optional<std::string> name = argument(*context.callSite);

                    if (name && !class_type(*context.constraint->scope, *metadata, *name)) {
                        report_new(context, *context.callSite->args.data[0], invalid_message(kind, *name));
                        set_error(context);

                        return true;
                    }

                    return false;
                }

                if (context.callSite->args.size == 0 || context.callSite->args.size > maximum_arguments()) {
                    return false;
                }

                const std::optional<std::string> name = argument(*context.callSite);
                const std::optional<Luau::TypeId> type = name ? class_type(*context.constraint->scope, *metadata, *name, kind) : std::nullopt;

                if (!type) {
                    if (name) {
                        report_new(context, *context.callSite->args.data[0], invalid_message(kind, *name));
                        set_error(context);
                    }

                    return name.has_value();
                }

                Luau::TypeId result = *type;

                if (returns_optional()) {
                    result = context.solver->arena->addType(Luau::UnionType{{context.solver->builtinTypes->nilType, result}});
                }

                Luau::asMutable(context.result)->ty.emplace<Luau::BoundTypePack>(context.solver->arena->addTypePack({result}));

                return true;
            }

            void refine(const Luau::MagicRefinementContext &context) override {
                if (kind != MagicKind::IsA || context.callSite->args.size != 1 || context.discriminantTypes.empty()) {
                    return;
                }

                const auto *index = context.callSite->func->as<Luau::AstExprIndexName>();
                const std::optional<std::string> name = argument(*context.callSite);

                if (!index || !name) {
                    return;
                }

                const std::optional<Luau::TypeId> type = class_type(*context.scope, *metadata, *name);
                const std::optional<Luau::LValue> lvalue = Luau::tryGetLValue(*index->expr);
                const std::optional<Luau::TypeId> discriminant = context.discriminantTypes[0];

                if (!type || !lvalue || !discriminant || !Luau::get<Luau::BlockedType>(*discriminant)) {
                    return;
                }

                Luau::asMutable(*discriminant)->ty.emplace<Luau::BoundType>(*type);
            }

          private:
            size_t maximum_arguments() const { return kind == MagicKind::Constructor || returns_optional() ? 2 : 1; }

            bool returns_optional() const {
                return kind == MagicKind::ChildClass || kind == MagicKind::ChildType || kind == MagicKind::AncestorClass || kind == MagicKind::AncestorType;
            }

            void set_error(const Luau::MagicFunctionCallContext &context) const {
                Luau::asMutable(context.result)->ty.emplace<Luau::BoundTypePack>(context.solver->builtinTypes->errorTypePack);
            }

            std::optional<Luau::WithPredicate<Luau::TypePackId>>
            old_is_a(Luau::TypeChecker &checker, const Luau::ScopePtr &scope, const Luau::AstExprCall &call, Luau::WithPredicate<Luau::TypePackId> predicate) const {

                if (call.args.size != 1) {
                    return std::nullopt;
                }

                const auto *index = call.func->as<Luau::AstExprIndexName>();
                const std::optional<std::string> name = argument(call);
                const std::optional<Luau::TypeId> type = name ? class_type(*scope, *metadata, *name) : std::nullopt;

                if (!index || !name || !type) {
                    return std::nullopt;
                }

                const std::optional<Luau::LValue> lvalue = Luau::tryGetLValue(*index->expr);
                const auto [parameters, tail] = Luau::flatten(predicate.type);
                const size_t offset = call.self ? 1 : 0;

                if (!lvalue || tail || parameters.size() < offset + 1) {
                    return std::nullopt;
                }

                checker.unify(parameters[offset], checker.stringType, scope, call.args.data[0]->location);

                return Luau::WithPredicate<Luau::TypePackId>{
                    checker.currentModule->internalTypes->addTypePack({checker.booleanType}),
                    {Luau::IsAPredicate{std::move(*lvalue), call.location, *type}},
                };
            }

            MagicKind kind;
            std::shared_ptr<const Metadata> metadata;
        };

        void attach(Luau::TypeId type, const std::shared_ptr<Luau::MagicFunction> &magic) {
            type = Luau::follow(type);

            if (auto *function = Luau::getMutable<Luau::FunctionType>(type)) {
                function->magic = magic;
            } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (Luau::TypeId part : intersection->parts) {
                    attach(part, magic);
                }
            }
        }

        template <typename Properties> void attach_property(Properties &properties, const char *name, const std::shared_ptr<Luau::MagicFunction> &magic) {
            const auto found = properties.find(name);

            if (found != properties.end() && found->second.readTy) {
                attach(*found->second.readTy, magic);
            }
        }

        void attach_global(Luau::GlobalTypes &globals, const char *global, const char *property, const std::shared_ptr<Luau::MagicFunction> &magic) {
            const std::optional<Luau::Binding> binding = Luau::tryGetGlobalBinding(globals, global);

            if (!binding) {
                return;
            }

            if (auto *table = Luau::getMutable<Luau::TableType>(Luau::follow(binding->typeId))) {
                attach_property(table->props, property, magic);
            }
        }

        void attach_extern(Luau::GlobalTypes &globals, const char *type, const char *property, const std::shared_ptr<Luau::MagicFunction> &magic) {
            const std::optional<Luau::TypeFun> found = globals.globalScope->lookupType(type);

            if (!found) {
                return;
            }

            if (auto *extern_type = Luau::getMutable<Luau::ExternType>(Luau::follow(found->type))) {
                attach_property(extern_type->props, property, magic);
            }
        }

        bool derives_from(Luau::TypeId type, Luau::TypeId base) {
            type = Luau::follow(type);
            base = Luau::follow(base);

            while (const auto *extern_type = Luau::get<Luau::ExternType>(type)) {
                if (type == base) {
                    return true;
                }

                if (!extern_type->parent) {
                    return false;
                }

                type = Luau::follow(*extern_type->parent);
            }

            return type == base;
        }

        void share_metatable(Luau::GlobalTypes &globals, const char *base_name) {
            const std::optional<Luau::TypeFun> base = globals.globalScope->lookupType(base_name);

            if (!base) {
                return;
            }

            const Luau::TypeId base_type = Luau::follow(base->type);
            const auto *base_extern = Luau::get<Luau::ExternType>(base_type);

            if (!base_extern || !base_extern->metatable) {
                return;
            }

            for (auto &[_, type_function] : globals.globalScope->exportedTypeBindings) {
                const Luau::TypeId type = Luau::follow(type_function.type);
                auto *extern_type = Luau::getMutable<Luau::ExternType>(type);

                if (extern_type && derives_from(type, base_type)) {
                    extern_type->metatable = base_extern->metatable;
                }
            }
        }

        void add_enum_type_aliases(Luau::GlobalTypes &globals) {
            auto &aliases = globals.globalScope->importedTypeBindings["Enum"];

            for (const auto &[name, type_function] : globals.globalScope->exportedTypeBindings) {
                if (name.size() <= 3 || name.rfind("Enum", 0) != 0 || name == "EnumItem") {
                    continue;
                }

                const std::string alias = "Enumeration" + name.substr(4);

                if (globals.globalScope->exportedTypeBindings.find(alias) == globals.globalScope->exportedTypeBindings.end()) {
                    aliases.insert_or_assign(name.substr(4), type_function);
                }
            }
        }

        void add_instance_children(Luau::GlobalTypes &globals, const Metadata &metadata) {
            const std::optional<Luau::TypeFun> instance = globals.globalScope->lookupType("Instance");

            if (!instance) {
                return;
            }

            auto *instance_type = Luau::getMutable<Luau::ExternType>(Luau::follow(instance->type));

            if (!instance_type) {
                return;
            }

            for (const NodeInfo &node : metadata.nodes) {
                if (instance_type->props.find(node.name) != instance_type->props.end()) {
                    continue;
                }

                const std::optional<Luau::TypeId> type = class_type(*globals.globalScope, metadata, node.class_name);

                if (type) {
                    instance_type->props.emplace(node.name, Luau::Property::readonly(*type));
                }
            }

            if (!metadata.nodes.empty()) {
                const auto parent = instance_type->props.find("Parent");

                if (parent != instance_type->props.end()) {
                    parent->second.setType(instance->type);
                }
            }
        }

        void add_instance_is_a(Luau::GlobalTypes &globals) {
            const std::optional<Luau::TypeFun> instance = globals.globalScope->lookupType("Instance");

            if (!instance) {
                return;
            }

            auto *instance_type = Luau::getMutable<Luau::ExternType>(Luau::follow(instance->type));

            if (!instance_type || instance_type->props.find("IsA") != instance_type->props.end()) {
                return;
            }

            const Luau::TypeId function =
                Luau::makeFunction(globals.globalTypes, instance->type, {globals.builtinTypes->stringType}, {globals.builtinTypes->booleanType});

            instance_type->props.emplace("IsA", Luau::Property::readonly(function));
        }

        void
        register_magic(Luau::GlobalTypes &globals, const std::shared_ptr<const Metadata> &metadata, MagicKind kind, const char *type, const char *property, bool global) {
            const auto magic = std::make_shared<MagicFunction>(kind, metadata);

            if (global) {
                attach_global(globals, type, property, magic);
            } else {
                attach_extern(globals, type, property, magic);
            }
        }
    } // namespace

    void register_roblox_magic(Luau::GlobalTypes &globals, const RobloxClass *classes, size_t class_count, const RobloxNode *nodes, size_t node_count) {
        auto metadata = std::make_shared<Metadata>();
        metadata->classes.reserve(class_count);
        metadata->nodes.reserve(node_count);

        for (size_t index = 0; index < class_count; ++index) {
            const auto name = std::string_view(reinterpret_cast<const char *>(classes[index].name.data), classes[index].name.length);
            metadata->classes.emplace(std::string(name), ClassInfo{classes[index].service != 0, classes[index].creatable != 0});
        }

        for (size_t index = 0; index < node_count; ++index) {
            NodeInfo node;
            node.name.assign(reinterpret_cast<const char *>(nodes[index].name.data), nodes[index].name.length);
            node.class_name.assign(reinterpret_cast<const char *>(nodes[index].class_name.data), nodes[index].class_name.length);
            metadata->nodes.push_back(std::move(node));
        }

        add_enum_type_aliases(globals);
        add_instance_children(globals, *metadata);
        share_metatable(globals, "Instance");
        share_metatable(globals, "Enum");
        share_metatable(globals, "EnumItem");
        add_instance_is_a(globals);

        register_magic(globals, metadata, MagicKind::Constructor, "Instance", "new", true);
        register_magic(globals, metadata, MagicKind::Service, "ServiceProvider", "GetService", false);
        register_magic(globals, metadata, MagicKind::IsA, "Instance", "IsA", false);
        register_magic(globals, metadata, MagicKind::ChildClass, "Instance", "FindFirstChildOfClass", false);
        register_magic(globals, metadata, MagicKind::ChildType, "Instance", "FindFirstChildWhichIsA", false);
        register_magic(globals, metadata, MagicKind::AncestorClass, "Instance", "FindFirstAncestorOfClass", false);
        register_magic(globals, metadata, MagicKind::AncestorType, "Instance", "FindFirstAncestorWhichIsA", false);
    }
} // namespace instar
