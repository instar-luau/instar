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

        enum class RobloxMagicKind {
            Constructor,
            GetService,
            IsA,
            FindFirstChildOfClass,
            FindFirstChildWhichIsA,
            FindFirstAncestorOfClass,
            FindFirstAncestorWhichIsA,
        };

        struct RobloxMetadata {
            std::unordered_map<std::string, RobloxClass> classes;
            std::vector<RobloxNode> nodes;
        };

        std::optional<std::string> string_argument(const Luau::AstExprCall &call, size_t index) {
            if (index >= call.args.size) {
                return std::nullopt;
            }

            const Luau::AstExprConstantString *value = call.args.data[index]->as<Luau::AstExprConstantString>();

            if (!value) {
                return std::nullopt;
            }

            return std::string(value->value.data, value->value.size);
        }

        std::optional<Luau::TypeId> lookup_class(
            const Luau::Scope &scope, const RobloxMetadata &metadata, std::string_view name, RobloxMagicKind kind) {
            auto classIt = metadata.classes.find(std::string(name));

            if (classIt == metadata.classes.end()) {
                return std::nullopt;
            }

            if (kind == RobloxMagicKind::Constructor && !classIt->second.creatable) {
                return std::nullopt;
            }

            if (kind == RobloxMagicKind::GetService && !classIt->second.service) {
                return std::nullopt;
            }

            const std::optional<Luau::TypeFun> type = scope.lookupType(std::string(name));

            if (!type) {
                return std::nullopt;
            }

            return type->type;
        }

        std::optional<Luau::TypeId> lookup_class(
            const Luau::Scope &scope, const RobloxMetadata &metadata, std::string_view name) {
            auto classIt = metadata.classes.find(std::string(name));

            if (classIt == metadata.classes.end()) {
                return std::nullopt;
            }

            const std::optional<Luau::TypeFun> type = scope.lookupType(std::string(name));

            if (!type) {
                return std::nullopt;
            }

            return type->type;
        }

        std::string invalid_class_message(RobloxMagicKind kind, std::string_view name) {
            switch (kind) {
            case RobloxMagicKind::Constructor:
                return "Instance.new does not support class '" + std::string(name) + "'";

            case RobloxMagicKind::GetService:
                return "GetService does not support service '" + std::string(name) + "'";

            case RobloxMagicKind::IsA:
                return "IsA does not support class '" + std::string(name) + "'";

            default:
                return "instance lookup does not support class '" + std::string(name) + "'";
            }
        }

        void report_old(Luau::TypeChecker &typechecker, const Luau::AstExpr &expression, std::string message) {
            typechecker.reportError(Luau::TypeError{expression.location, Luau::GenericError{std::move(message)}});
        }

        void report_new(
            const Luau::MagicFunctionCallContext &context, const Luau::AstExpr &expression, std::string message) {

            if (FFlag::LuauCyclicRequireTypeInference) {
                context.solver->reportError(
                    Luau::GenericError{std::move(message)}, expression.location, *context.constraint->moduleName);
            } else {
                context.solver->DEPRECATED_reportError(
                    Luau::TypeError{expression.location, Luau::GenericError{std::move(message)}});
            }
        }

        Luau::TypePackId old_error_pack(Luau::TypeChecker &typechecker) {
            return typechecker.currentModule->internalTypes->addTypePack({typechecker.builtinTypes->errorType});
        }

        void new_error_pack(const Luau::MagicFunctionCallContext &context) {
            Luau::asMutable(context.result)
                ->ty.emplace<Luau::BoundTypePack>(context.solver->builtinTypes->errorTypePack);
        }

        class RobloxMagic final : public Luau::MagicFunction {
          public:
            RobloxMagic(RobloxMagicKind kind, std::shared_ptr<const RobloxMetadata> metadata)
                : kind(kind), metadata(std::move(metadata)) {}

            std::optional<Luau::WithPredicate<Luau::TypePackId>> handleOldSolver(Luau::TypeChecker &typechecker,
                const Luau::ScopePtr &scope, const Luau::AstExprCall &call,
                Luau::WithPredicate<Luau::TypePackId> withPredicate) override {

                if (kind == RobloxMagicKind::IsA) {
                    return handle_old_isa(typechecker, scope, call, std::move(withPredicate));
                }

                const size_t maximum_arguments = kind == RobloxMagicKind::Constructor ? 2
                                                 : kind == RobloxMagicKind::FindFirstChildOfClass ||
                                                         kind == RobloxMagicKind::FindFirstChildWhichIsA ||
                                                         kind == RobloxMagicKind::FindFirstAncestorOfClass ||
                                                         kind == RobloxMagicKind::FindFirstAncestorWhichIsA
                                                     ? 2
                                                     : 1;

                if (call.args.size == 0 || call.args.size > maximum_arguments) {
                    return std::nullopt;
                }

                const std::optional<std::string> name = string_argument(call, 0);

                if (!name) {
                    return std::nullopt;
                }

                const std::optional<Luau::TypeId> type = lookup_class(*scope, *metadata, *name, kind);

                if (!type) {
                    report_old(typechecker, *call.args.data[0], invalid_class_message(kind, *name));

                    return Luau::WithPredicate<Luau::TypePackId>{old_error_pack(typechecker)};
                }

                if (!validate_old_arguments(typechecker, scope, call, withPredicate.type)) {
                    return std::nullopt;
                }

                Luau::TypeId result = *type;

                if (kind == RobloxMagicKind::FindFirstChildOfClass || kind == RobloxMagicKind::FindFirstChildWhichIsA ||
                    kind == RobloxMagicKind::FindFirstAncestorOfClass ||
                    kind == RobloxMagicKind::FindFirstAncestorWhichIsA) {
                    result = typechecker.currentModule->internalTypes->addType(
                        Luau::UnionType{{typechecker.builtinTypes->nilType, result}});
                }

                return Luau::WithPredicate<Luau::TypePackId>{
                    typechecker.currentModule->internalTypes->addTypePack({result})};
            }

            bool infer(const Luau::MagicFunctionCallContext &context) override {
                if (kind == RobloxMagicKind::IsA) {
                    if (context.callSite->args.size == 1) {
                        if (const std::optional<std::string> name = string_argument(*context.callSite, 0);
                            name && !lookup_class(*context.constraint->scope, *metadata, *name)) {
                            report_new(context, *context.callSite->args.data[0], invalid_class_message(kind, *name));
                            new_error_pack(context);

                            return true;
                        }
                    }

                    return false;
                }

                const size_t maximum_arguments = kind == RobloxMagicKind::Constructor ? 2
                                                 : kind == RobloxMagicKind::FindFirstChildOfClass ||
                                                         kind == RobloxMagicKind::FindFirstChildWhichIsA ||
                                                         kind == RobloxMagicKind::FindFirstAncestorOfClass ||
                                                         kind == RobloxMagicKind::FindFirstAncestorWhichIsA
                                                     ? 2
                                                     : 1;

                if (context.callSite->args.size == 0 || context.callSite->args.size > maximum_arguments) {
                    return false;
                }

                const std::optional<std::string> name = string_argument(*context.callSite, 0);

                if (!name) {
                    return false;
                }

                const std::optional<Luau::TypeId> type =
                    lookup_class(*context.constraint->scope, *metadata, *name, kind);

                if (!type) {
                    report_new(context, *context.callSite->args.data[0], invalid_class_message(kind, *name));
                    new_error_pack(context);

                    return true;
                }

                Luau::TypeId result = *type;

                if (kind == RobloxMagicKind::FindFirstChildOfClass || kind == RobloxMagicKind::FindFirstChildWhichIsA ||
                    kind == RobloxMagicKind::FindFirstAncestorOfClass ||
                    kind == RobloxMagicKind::FindFirstAncestorWhichIsA) {
                    result = context.solver->arena->addType(
                        Luau::UnionType{{context.solver->builtinTypes->nilType, result}});
                }

                const Luau::TypePackId result_pack = context.solver->arena->addTypePack({result});
                Luau::asMutable(context.result)->ty.emplace<Luau::BoundTypePack>(result_pack);

                return true;
            }

            void refine(const Luau::MagicRefinementContext &context) override {
                if (kind != RobloxMagicKind::IsA || context.callSite->args.size != 1 ||
                    context.discriminantTypes.empty()) {
                    return;
                }

                const Luau::AstExprIndexName *index = context.callSite->func->as<Luau::AstExprIndexName>();
                const std::optional<std::string> name = string_argument(*context.callSite, 0);

                if (!index || !name) {
                    return;
                }

                const std::optional<Luau::LValue> lvalue = Luau::tryGetLValue(*index->expr);

                if (!lvalue) {
                    return;
                }

                const std::optional<Luau::TypeId> type = lookup_class(*context.scope, *metadata, *name);

                if (!type) {
                    return;
                }

                const std::optional<Luau::TypeId> discriminant = context.discriminantTypes[0];

                if (!discriminant || !Luau::get<Luau::BlockedType>(*discriminant)) {
                    return;
                }

                Luau::asMutable(*discriminant)->ty.emplace<Luau::BoundType>(*type);
            }

          private:
            bool validate_old_arguments(Luau::TypeChecker &typechecker, const Luau::ScopePtr &scope,
                const Luau::AstExprCall &call, Luau::TypePackId arguments) const {
                const auto [parameters, tail] = Luau::flatten(arguments);
                const size_t self_offset = call.self ? 1 : 0;

                if (tail || parameters.size() < self_offset + call.args.size) {
                    return false;
                }

                const size_t first_argument = self_offset;

                typechecker.unify(
                    parameters[first_argument], typechecker.stringType, scope, call.args.data[0]->location);

                if (call.args.size == 2) {
                    if (kind != RobloxMagicKind::Constructor && kind != RobloxMagicKind::FindFirstChildOfClass &&
                        kind != RobloxMagicKind::FindFirstChildWhichIsA &&
                        kind != RobloxMagicKind::FindFirstAncestorOfClass &&
                        kind != RobloxMagicKind::FindFirstAncestorWhichIsA) {
                        return false;
                    }

                    if (kind == RobloxMagicKind::Constructor) {
                        const std::optional<Luau::TypeFun> instance_type = scope->lookupType("Instance");

                        if (!instance_type) {
                            return false;
                        }

                        const Luau::TypeId optional_instance = typechecker.currentModule->internalTypes->addType(
                            Luau::UnionType{{typechecker.builtinTypes->nilType, instance_type->type}});

                        typechecker.unify(
                            parameters[first_argument + 1], optional_instance, scope, call.args.data[1]->location);
                    } else {
                        const Luau::TypeId optional_boolean = typechecker.currentModule->internalTypes->addType(
                            Luau::UnionType{{typechecker.builtinTypes->nilType, typechecker.booleanType}});

                        typechecker.unify(
                            parameters[first_argument + 1], optional_boolean, scope, call.args.data[1]->location);
                    }
                }

                return true;
            }

            std::optional<Luau::WithPredicate<Luau::TypePackId>> handle_old_isa(Luau::TypeChecker &typechecker,
                const Luau::ScopePtr &scope, const Luau::AstExprCall &call,
                Luau::WithPredicate<Luau::TypePackId> withPredicate) const {

                if (call.args.size != 1) {
                    return std::nullopt;
                }

                const Luau::AstExprIndexName *index = call.func->as<Luau::AstExprIndexName>();
                const std::optional<std::string> name = string_argument(call, 0);

                if (!index || !name) {
                    return std::nullopt;
                }

                const std::optional<Luau::TypeId> type = lookup_class(*scope, *metadata, *name);

                if (!type) {
                    report_old(typechecker, *call.args.data[0], invalid_class_message(kind, *name));

                    return Luau::WithPredicate<Luau::TypePackId>{old_error_pack(typechecker)};
                }

                const std::optional<Luau::LValue> lvalue = Luau::tryGetLValue(*index->expr);

                if (!lvalue) {
                    return std::nullopt;
                }

                const auto [parameters, tail] = Luau::flatten(withPredicate.type);
                const size_t self_offset = call.self ? 1 : 0;

                if (tail || parameters.size() < self_offset + 1) {
                    return std::nullopt;
                }

                typechecker.unify(parameters[self_offset], typechecker.stringType, scope, call.args.data[0]->location);

                const Luau::TypePackId result =
                    typechecker.currentModule->internalTypes->addTypePack({typechecker.booleanType});

                return Luau::WithPredicate<Luau::TypePackId>{
                    result, {Luau::IsAPredicate{std::move(*lvalue), call.location, *type}}};
            }

            RobloxMagicKind kind;
            std::shared_ptr<const RobloxMetadata> metadata;
        };

        void attach(Luau::TypeId type, const std::shared_ptr<Luau::MagicFunction> &magic) {
            type = Luau::follow(type);

            if (Luau::FunctionType *function = Luau::getMutable<Luau::FunctionType>(type)) {
                function->magic = magic;

                return;
            }

            if (const Luau::IntersectionType *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (Luau::TypeId part : intersection->parts) {
                    attach(part, magic);
                }
            }
        }

        template <typename Properties>
        void attach_property(
            Properties &properties, const char *name, const std::shared_ptr<Luau::MagicFunction> &magic) {
            auto property = properties.find(name);

            if (property != properties.end() && property->second.readTy) {
                attach(*property->second.readTy, magic);
            }
        }

        void attach_global_property(Luau::GlobalTypes &globals, const char *global, const char *property,
            const std::shared_ptr<Luau::MagicFunction> &magic) {
            const std::optional<Luau::Binding> binding = Luau::tryGetGlobalBinding(globals, global);

            if (!binding) {
                return;
            }

            if (Luau::TableType *table = Luau::getMutable<Luau::TableType>(Luau::follow(binding->typeId))) {
                attach_property(table->props, property, magic);
            }
        }

        void attach_extern_property(Luau::GlobalTypes &globals, const char *type, const char *property,
            const std::shared_ptr<Luau::MagicFunction> &magic) {
            const std::optional<Luau::TypeFun> type_function = globals.globalScope->lookupType(type);

            if (!type_function) {
                return;
            }

            if (Luau::ExternType *extern_type = Luau::getMutable<Luau::ExternType>(Luau::follow(type_function->type))) {
                attach_property(extern_type->props, property, magic);
            }
        }

        bool derives_from(Luau::TypeId type, Luau::TypeId base) {
            type = Luau::follow(type);
            base = Luau::follow(base);

            while (const Luau::ExternType *extern_type = Luau::get<Luau::ExternType>(type)) {
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
            const std::optional<Luau::TypeFun> base_function = globals.globalScope->lookupType(base_name);

            if (!base_function) {
                return;
            }

            const Luau::TypeId base_type = Luau::follow(base_function->type);
            const Luau::ExternType *base_extern = Luau::get<Luau::ExternType>(base_type);

            if (!base_extern || !base_extern->metatable) {
                return;
            }

            for (auto &[_, type_function] : globals.globalScope->exportedTypeBindings) {
                const Luau::TypeId type = Luau::follow(type_function.type);
                Luau::ExternType *extern_type = Luau::getMutable<Luau::ExternType>(type);

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

                const std::string enumeration_name = "Enumeration" + name.substr(4);

                if (globals.globalScope->exportedTypeBindings.find(enumeration_name) !=
                    globals.globalScope->exportedTypeBindings.end()) {
                    aliases.insert_or_assign(name.substr(4), type_function);
                }
            }
        }

        void add_instance_children(Luau::GlobalTypes &globals, const RobloxMetadata &metadata) {
            const std::optional<Luau::TypeFun> instance_function = globals.globalScope->lookupType("Instance");

            if (!instance_function) {
                return;
            }

            Luau::ExternType *instance_type = Luau::getMutable<Luau::ExternType>(Luau::follow(instance_function->type));

            if (!instance_type) {
                return;
            }

            for (const RobloxNode &node : metadata.nodes) {
                if (instance_type->props.find(node.name) != instance_type->props.end()) {
                    continue;
                }

                const std::optional<Luau::TypeId> type = lookup_class(*globals.globalScope, metadata, node.class_name);

                if (type) {
                    instance_type->props.emplace(node.name, Luau::Property::readonly(*type));
                }
            }

            if (metadata.nodes.empty()) {
                return;
            }

            const auto parent = instance_type->props.find("Parent");

            if (parent != instance_type->props.end()) {
                parent->second.setType(instance_function->type);
            }
        }

        void add_instance_is_a(Luau::GlobalTypes &globals) {
            const std::optional<Luau::TypeFun> instance_function = globals.globalScope->lookupType("Instance");

            if (!instance_function) {
                return;
            }

            Luau::ExternType *instance_type = Luau::getMutable<Luau::ExternType>(Luau::follow(instance_function->type));

            if (!instance_type || instance_type->props.find("IsA") != instance_type->props.end()) {
                return;
            }

            const Luau::TypeId function = Luau::makeFunction(globals.globalTypes, instance_function->type,
                {globals.builtinTypes->stringType}, {globals.builtinTypes->booleanType});

            instance_type->props.emplace("IsA", Luau::Property::readonly(function));
        }

    } // namespace

    void register_roblox_magic(
        Luau::GlobalTypes &globals, const std::vector<RobloxClass> &classes, const std::vector<RobloxNode> &nodes) {
        auto metadata = std::make_shared<RobloxMetadata>();
        metadata->classes.reserve(classes.size());
        metadata->nodes = nodes;

        for (const RobloxClass &value : classes) {
            metadata->classes.insert_or_assign(value.name, value);
        }

        add_enum_type_aliases(globals);
        add_instance_children(globals, *metadata);
        share_metatable(globals, "Instance");
        share_metatable(globals, "Enum");
        share_metatable(globals, "EnumItem");
        add_instance_is_a(globals);

        attach_global_property(
            globals, "Instance", "new", std::make_shared<RobloxMagic>(RobloxMagicKind::Constructor, metadata));

        attach_extern_property(globals, "ServiceProvider", "GetService",
            std::make_shared<RobloxMagic>(RobloxMagicKind::GetService, metadata));

        attach_extern_property(
            globals, "Instance", "IsA", std::make_shared<RobloxMagic>(RobloxMagicKind::IsA, metadata));

        attach_extern_property(globals, "Instance", "FindFirstChildOfClass",
            std::make_shared<RobloxMagic>(RobloxMagicKind::FindFirstChildOfClass, metadata));

        attach_extern_property(globals, "Instance", "FindFirstChildWhichIsA",
            std::make_shared<RobloxMagic>(RobloxMagicKind::FindFirstChildWhichIsA, metadata));

        attach_extern_property(globals, "Instance", "FindFirstAncestorOfClass",
            std::make_shared<RobloxMagic>(RobloxMagicKind::FindFirstAncestorOfClass, metadata));

        attach_extern_property(globals, "Instance", "FindFirstAncestorWhichIsA",
            std::make_shared<RobloxMagic>(RobloxMagicKind::FindFirstAncestorWhichIsA, metadata));
    }

} // namespace instar
