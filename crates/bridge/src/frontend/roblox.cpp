#include "instar-bridge/src/boundary.rs.h"

#include "roblox.hpp"

#include "Luau/Ast.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/ConstraintSolver.h"
#include "Luau/Error.h"
#include "Luau/Frontend.h"
#include "Luau/LValue.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "Luau/TypePack.h"
#include "Luau/TypeUtils.h"

#include <cstdint>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

LUAU_FASTFLAG(LuauCyclicRequireTypeInference)

namespace instar {
    namespace {
        enum class MagicKind : std::uint8_t {
            Constructor,
            Service,
            IsA,
            ChildClass,
            ChildType,
            AncestorClass,
            AncestorType,
        };

        struct ClassInfo {
            Luau::TypeId type;
            bool service;
            bool creatable;
        };

        struct Metadata {
            std::unordered_map<std::string, ClassInfo> classes;
        };

        std::optional<std::string> argument(const Luau::AstExprCall &call) {
            if (call.args.size == 0) {
                return std::nullopt;
            }

            const auto *value = call.args.data[0]->as<Luau::AstExprConstantString>();

            return value ? std::optional<std::string>(std::string(value->value.data, value->value.size)) : std::nullopt;
        }

        std::optional<Luau::TypeId>
        class_type(const Metadata &metadata, const std::string &name, MagicKind kind = MagicKind::IsA) {
            const auto found = metadata.classes.find(name);

            if (found == metadata.classes.end() || (kind == MagicKind::Constructor && !found->second.creatable) ||
                (kind == MagicKind::Service && !found->second.service)) {
                return std::nullopt;
            }

            return found->second.type;
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

        bool derives_from(Luau::TypeId type, Luau::TypeId base);

        void report_new(
            const Luau::MagicFunctionCallContext &context, const Luau::AstExpr &expression, std::string message
        ) {
            if (FFlag::LuauCyclicRequireTypeInference) {
                context.solver->reportError(
                    Luau::GenericError{std::move(message)},
                    expression.location,
                    *context.constraint->moduleName
                );
            } else {
                context.solver->DEPRECATED_reportError(
                    Luau::TypeError{expression.location, Luau::GenericError{std::move(message)}}
                );
            }
        }

        class MagicFunction final : public Luau::MagicFunction {
          public:
            MagicFunction(MagicKind kind, std::shared_ptr<const Metadata> metadata)
                : kind(kind), metadata(std::move(metadata)) {}

            std::optional<Luau::WithPredicate<Luau::TypePackId>> handleOldSolver(
                Luau::TypeChecker &, const Luau::ScopePtr &, const Luau::AstExprCall &,
                Luau::WithPredicate<Luau::TypePackId>
            ) override {
                throw std::logic_error("old solver is not supported");
            }

            bool infer(const Luau::MagicFunctionCallContext &context) override {
                if (kind == MagicKind::IsA && context.callSite->args.size == 1) {
                    const std::optional<std::string> name = argument(*context.callSite);

                    if (name && !class_type(*metadata, *name)) {
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
                const std::optional<Luau::TypeId> type = name ? class_type(*metadata, *name, kind) : std::nullopt;

                if (!type) {
                    if (name) {
                        report_new(context, *context.callSite->args.data[0], invalid_message(kind, *name));
                        set_error(context);
                    }

                    return name.has_value();
                }

                Luau::TypeId result = *type;

                if (kind == MagicKind::Service && context.callSite->self) {
                    if (const auto receiver = Luau::first(context.arguments)) {
                        if (const auto *instance = Luau::get<Luau::ExternType>(Luau::follow(*receiver))) {
                            Luau::TypeId service = nullptr;

                            for (const auto &[name, property] : instance->props) {
                                if (name != "Parent" && property.readTy && derives_from(*property.readTy, *type)) {
                                    if (!service) {
                                        service = *property.readTy;
                                    } else if (service != *property.readTy) {
                                        service = context.solver->arena->addType(
                                            Luau::UnionType{{service, *property.readTy}}
                                        );
                                    }
                                }
                            }

                            if (service) {
                                result = service;
                            }
                        }
                    }
                }

                if (returns_optional()) {
                    result = context.solver->arena->addType(
                        Luau::UnionType{{context.solver->builtinTypes->nilType, result}}
                    );
                }

                Luau::asMutable(context.result)
                    ->ty.emplace<Luau::BoundTypePack>(context.solver->arena->addTypePack({result}));

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

                const std::optional<Luau::TypeId> type = class_type(*metadata, *name);
                const std::optional<Luau::LValue> lvalue = Luau::tryGetLValue(*index->expr);
                const std::optional<Luau::TypeId> discriminant = context.discriminantTypes[0];

                if (!type || !lvalue || !discriminant || !Luau::get<Luau::BlockedType>(*discriminant)) {
                    return;
                }

                Luau::asMutable(*discriminant)->ty.emplace<Luau::BoundType>(*type);
            }

          private:
            size_t maximum_arguments() const {
                return kind == MagicKind::Constructor || kind == MagicKind::ChildType ? 2 : 1;
            }

            bool returns_optional() const {
                return kind == MagicKind::ChildClass || kind == MagicKind::ChildType ||
                       kind == MagicKind::AncestorClass || kind == MagicKind::AncestorType;
            }

            void set_error(const Luau::MagicFunctionCallContext &context) const {
                Luau::asMutable(context.result)
                    ->ty.emplace<Luau::BoundTypePack>(context.solver->builtinTypes->errorTypePack);
            }

            MagicKind kind;
            std::shared_ptr<const Metadata> metadata;
        };

        void attach(Luau::TypeId type, const std::shared_ptr<Luau::MagicFunction> &magic, const char *tag) {
            type = Luau::follow(type);

            if (auto *function = Luau::getMutable<Luau::FunctionType>(type)) {
                if (magic) {
                    function->magic = magic;
                }

                function->tags.emplace_back(tag);
            } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (Luau::TypeId part : intersection->parts) {
                    attach(part, magic, tag);
                }
            }
        }

        template <typename Properties>
        void attach_property(
            Properties &properties, const char *name, const std::shared_ptr<Luau::MagicFunction> &magic, const char *tag
        ) {
            const auto found = properties.find(name);

            if (found != properties.end()) {
                const auto readable = found->second.readTy;

                if (readable) {
                    attach(*readable, magic, tag);
                }
            }
        }

        void attach_global(
            Luau::GlobalTypes &globals, const char *global, const std::shared_ptr<Luau::MagicFunction> &magic,
            const char *property, const char *tag
        ) {
            const std::optional<Luau::Binding> binding = Luau::tryGetGlobalBinding(globals, global);

            if (!binding) {
                return;
            }

            if (auto *table = Luau::getMutable<Luau::TableType>(Luau::follow(binding->typeId))) {
                attach_property(table->props, property, magic, tag);
            }
        }

        void attach_extern(
            Luau::GlobalTypes &globals, const char *type, const std::shared_ptr<Luau::MagicFunction> &magic,
            const char *property, const char *tag
        ) {
            const std::optional<Luau::TypeFun> found = globals.globalScope->lookupType(type);

            if (!found) {
                return;
            }

            if (auto *extern_type = Luau::getMutable<Luau::ExternType>(Luau::follow(found->type))) {
                attach_property(extern_type->props, property, magic, tag);
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
            const auto enum_item = globals.globalScope->lookupType("EnumItem");

            if (!enum_item) {
                return;
            }

            auto &aliases = globals.globalScope->importedTypeBindings["Enum"];

            for (const auto &[name, type_function] : globals.globalScope->exportedTypeBindings) {
                if (name.size() > 4 && name.rfind("Enum", 0) == 0 && name != "EnumItem" &&
                    derives_from(type_function.type, enum_item->type)) {
                    aliases.insert_or_assign(name.substr(4), type_function);
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

            const Luau::TypeId function = Luau::makeFunction(
                globals.globalTypes,
                instance->type,
                {globals.builtinTypes->stringType},
                {globals.builtinTypes->booleanType}
            );

            instance_type->props.emplace("IsA", Luau::Property::readonly(function));
        }

        void register_magic(
            Luau::GlobalTypes &globals, const std::shared_ptr<const Metadata> &metadata, MagicKind kind,
            const char *type, const char *property, bool global
        ) {
            const auto magic = std::make_shared<MagicFunction>(kind, metadata);

            const char *tag = kind == MagicKind::Constructor ? "instar.creatable"
                              : kind == MagicKind::Service   ? "instar.service"
                                                             : "instar.class";

            if (global) {
                attach_global(globals, type, magic, property, tag);
            } else {
                attach_extern(globals, type, magic, property, tag);
            }
        }
    } // namespace

    void register_roblox_magic(Luau::GlobalTypes &globals, rust::Slice<const NativeClass> classes) {
        auto metadata = std::make_shared<Metadata>();
        metadata->classes.reserve(classes.size());
        auto base = globals.globalScope->lookupType("Object");

        if (!base) {
            base = globals.globalScope->lookupType("Instance");
        }

        if (!base || !Luau::get<Luau::ExternType>(Luau::follow(base->type))) {
            throw std::invalid_argument("Roblox class magic requires Object or Instance declarations");
        }

        for (const auto &[name, type_function] : globals.globalScope->exportedTypeBindings) {
            const auto type = Luau::follow(type_function.type);

            if (Luau::get<Luau::ExternType>(type) && derives_from(type, base->type)) {
                metadata->classes.emplace(name, ClassInfo{type, false, false});
                Luau::getMutable<Luau::ExternType>(type)->tags.emplace_back("instar.class");
            }
        }

        std::unordered_set<std::string> tagged;

        for (size_t index = 0; index < classes.size(); ++index) {
            if (classes[index].name.empty()) {
                throw std::invalid_argument("empty Roblox class name");
            }

            const std::string name(classes[index].name);
            const auto type = metadata->classes.find(name);

            if (type == metadata->classes.end()) {
                throw std::invalid_argument("Roblox class has no declaration: " + name);
            }

            if (!tagged.insert(name).second) {
                throw std::invalid_argument("duplicate Roblox class flags: " + name);
            }

            type->second.service = classes[index].service;
            type->second.creatable = classes[index].creatable;
            auto *klass = Luau::getMutable<Luau::ExternType>(type->second.type);

            for (const NativeProperty &access : classes[index].properties) {
                const auto property = klass->props.find(std::string(access.name));

                if (property == klass->props.end()) {
                    throw std::invalid_argument(
                        "Roblox property metadata has no declaration: " + name + "." + std::string(access.name)
                    );
                }

                if (!access.read && !access.write) {
                    klass->props.erase(property);
                } else if (!access.read) {
                    property->second.readTy.reset();
                } else if (!access.write) {
                    property->second.writeTy.reset();
                }
            }

            if (classes[index].service) {
                klass->tags.emplace_back("instar.service");
            }

            if (classes[index].creatable) {
                klass->tags.emplace_back("instar.creatable");
            }
        }

        add_enum_type_aliases(globals);
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
        attach_extern(globals, "Instance", nullptr, "FindFirstChild", "instar.child");
        attach_extern(globals, "Instance", nullptr, "WaitForChild", "instar.child");
    }

} // namespace instar
