#include "engine.hpp"

#include "Luau/Ast.h"
#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Common.h"
#include "Luau/Config.h"
#include "Luau/ConfigResolver.h"
#include "Luau/ConstraintSolver.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Flags.h"
#include "Luau/Frontend.h"
#include "Luau/LValue.h"
#include "Luau/Linter.h"
#include "Luau/LuauConfig.h"
#include "Luau/Predicate.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"
#include "Luau/TypeCheckLimits.h"
#include "Luau/TypeFwd.h"
#include "Luau/TypeInfer.h"
#include "Luau/TypePack.h"
#include "Luau/Variant.h"

#include <algorithm>
#include <cstddef>
#include <filesystem>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

LUAU_FASTINT(LuauTarjanChildLimit)

namespace {

    std::string normalize(const std::string &name) {
        return std::filesystem::path(name).lexically_normal().generic_string();
    }

    instar::Range range(const Luau::Location &location) {
        return {
            {location.begin.line, location.begin.column},
            {location.end.line, location.end.column},
        };
    }

    Luau::Location location(const instar::Range &range) {
        return {
            {range.begin.line, range.begin.column},
            {range.end.line, range.end.column},
        };
    }

    class Sources final : public Luau::FileResolver {
        public:
        Sources(const std::vector<instar::Module> &modules, instar::ModuleResolver resolver)
            : resolver(std::move(resolver)) {
            set_modules(modules);
        }

        void set_modules(const std::vector<instar::Module> &modules) {
            sources.clear();

            for (const auto &module : modules) {
                sources.emplace(normalize(module.name), module.source);
            }
        }

        std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override {
            auto iterator = sources.find(normalize(name));

            if (iterator == sources.end()) {
                return std::nullopt;
            }

            return Luau::SourceCode{iterator->second, Luau::SourceCode::Module};
        }

        std::optional<Luau::ModuleInfo> resolveModule(
            const Luau::ModuleInfo *from, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override {

            if (!from || !resolver) {
                return std::nullopt;
            }

            auto literal = expression->as<Luau::AstExprConstantString>();

            if (!literal) {
                return std::nullopt;
            }

            std::string specifier(literal->value.data, literal->value.size);
            auto target = resolver(from->name, range(expression->location), specifier);

            return target ? std::optional<Luau::ModuleInfo>{Luau::ModuleInfo{normalize(*target)}} : std::nullopt;
        }

        private:
        std::map<std::string, std::string> sources;
        instar::ModuleResolver resolver;
    };

    bool is_configuration(const std::string &path) {
        auto name = path.substr(path.find_last_of("/\\") + 1);

        return name == ".config.luau" || name == "config.luau";
    }

    class Configurations final : public Luau::ConfigResolver {
        public:
        explicit Configurations(std::vector<instar::Configuration> configurations)
            : configurations(std::move(configurations)) {}

        void set_configurations(std::vector<instar::Configuration> values) {
            configurations = std::move(values);
            cache.clear();
        }

        const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override {
            auto [iterator, inserted] = cache.try_emplace(name);

            if (!inserted) {
                return iterator->second;
            }

            Luau::ConfigOptions options;
            options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};

            for (const auto &configuration : configurations) {
                if (!configuration.module.empty() && normalize(configuration.module) != normalize(name)) {
                    continue;
                }

                if (is_configuration(configuration.path)) {
                    Luau::InterruptCallbacks callbacks{};
                    Luau::ConfigOptions::AliasOptions aliases{configuration.path, true};
                    Luau::extractLuauConfig(configuration.source, iterator->second, aliases, std::move(callbacks));
                } else {
                    Luau::parseConfig(configuration.source, iterator->second, options);
                }
            }

            return iterator->second;
        }

        private:
        std::vector<instar::Configuration> configurations;
        mutable std::map<std::string, Luau::Config> cache;
    };

    void initialize() {
        static std::once_flag flags;

        std::call_once(flags, [] {
            setLuauFlagsDefault();

            if (FInt::LuauTarjanChildLimit > 0 && FInt::LuauTarjanChildLimit < 15000) {
                FInt::LuauTarjanChildLimit.value = 15000;
            }
        });
    }

    struct RobloxInstances final : Luau::ClassUserData {
        std::map<std::string, Luau::TypeId> children;
    };

    std::optional<Luau::TypeId> instance_type(const Luau::Scope &scope, const std::string &name) {
        auto base = scope.lookupType("Object");

        if (!base) {
            base = scope.lookupType("Instance");
        }

        auto target = scope.lookupType(name);

        if (!base || !target) {
            return std::nullopt;
        }

        auto parent = Luau::get<Luau::ExternType>(Luau::follow(base->type));
        auto type = Luau::get<Luau::ExternType>(Luau::follow(target->type));

        return parent && type && Luau::isSubclass(type, parent) ? std::optional<Luau::TypeId>{target->type}
                                                                : std::nullopt;
    }

    void report_magic_error(
        const Luau::MagicFunctionCallContext &context, const Luau::Location &location, const std::string &message) {

        if (context.constraint->moduleName) {
            context.solver->reportError(Luau::GenericError{message}, location, *context.constraint->moduleName);
        } else {
            context.solver->DEPRECATED_reportError(Luau::TypeError{location, Luau::GenericError{message}});
        }
    }

    struct RobloxPredicate final : Luau::MagicFunction {
        std::optional<Luau::WithPredicate<Luau::TypePackId>> handleOldSolver(Luau::TypeChecker &checker,
            const Luau::ScopePtr &scope, const Luau::AstExprCall &call,
            Luau::WithPredicate<Luau::TypePackId>) override {

            if (!call.self || call.args.size != 1) {
                return std::nullopt;
            }

            auto member = call.func->as<Luau::AstExprIndexName>();
            auto literal = call.args.data[0]->as<Luau::AstExprConstantString>();

            if (!member || !literal) {
                return std::nullopt;
            }

            auto value = Luau::tryGetLValue(*member->expr);
            std::string name(literal->value.data, literal->value.size);
            auto type = instance_type(*scope, name);

            if (!type) {
                checker.reportError(
                    Luau::TypeError{literal->location, Luau::GenericError{"Unknown instance class '" + name + "'"}});
            }

            if (!value || !type) {
                return std::nullopt;
            }

            return Luau::WithPredicate<Luau::TypePackId>{
                checker.currentModule->internalTypes->addTypePack({checker.booleanType}),
                {Luau::IsAPredicate{std::move(*value), call.location, *type}},
            };
        }

        bool infer(const Luau::MagicFunctionCallContext &context) override {
            if (context.callSite->args.size == 1) {
                if (auto literal = context.callSite->args.data[0]->as<Luau::AstExprConstantString>()) {
                    std::string name(literal->value.data, literal->value.size);

                    if (!instance_type(*context.solver->rootScope, name)) {
                        report_magic_error(context, literal->location, "Unknown instance class '" + name + "'");
                    }
                }
            }

            return false;
        }

        void refine(const Luau::MagicRefinementContext &context) override {
            if (!context.callSite->self || context.callSite->args.size != 1 || context.discriminantTypes.empty()) {
                return;
            }

            auto literal = context.callSite->args.data[0]->as<Luau::AstExprConstantString>();

            if (!literal || !context.discriminantTypes[0]) {
                return;
            }

            auto type = instance_type(*context.scope, std::string(literal->value.data, literal->value.size));

            if (type) {
                Luau::asMutable(*context.discriminantTypes[0])->ty.emplace<Luau::BoundType>(*type);
            }
        }
    };

    enum class RobloxLookupKind {
        child,
        class_type,
        service,
        constructor,
    };

    struct RobloxLookup final : Luau::MagicFunction {
        RobloxLookupKind kind;

        explicit RobloxLookup(RobloxLookupKind kind) : kind(kind) {}

        std::optional<Luau::TypeId> lookup(
            const Luau::Scope &scope, const Luau::AstExprCall &call, std::optional<Luau::TypeId> receiver) {

            if (!call.args.size) {
                return std::nullopt;
            }

            auto literal = call.args.data[0]->as<Luau::AstExprConstantString>();

            if (!literal) {
                return std::nullopt;
            }

            std::string name(literal->value.data, literal->value.size);
            auto result = instance_type(scope, name);

            if (kind != RobloxLookupKind::child) {
                if (!result) {
                    return std::nullopt;
                }

                auto type = Luau::get<Luau::ExternType>(Luau::follow(*result));

                auto tagged = [&](const char *tag) {
                    return std::find(type->tags.begin(), type->tags.end(), tag) != type->tags.end();
                };

                if ((kind == RobloxLookupKind::service && tagged("RobloxClass") && !tagged("RobloxService")) ||
                    (kind == RobloxLookupKind::constructor && tagged("RobloxNotCreatable"))) {
                    return std::nullopt;
                }
            }

            if (receiver && (kind == RobloxLookupKind::child || kind == RobloxLookupKind::service)) {
                if (auto type = Luau::get<Luau::ExternType>(Luau::follow(*receiver))) {
                    if (auto mapped = std::dynamic_pointer_cast<RobloxInstances>(type->userData)) {
                        if (kind == RobloxLookupKind::child) {
                            auto child = mapped->children.find(name);

                            if (child != mapped->children.end()) {
                                return child->second;
                            }
                        } else {
                            for (const auto &child : mapped->children) {
                                if (Luau::get<Luau::ExternType>(child.second)->name == name) {
                                    return child.second;
                                }
                            }
                        }
                    }
                }
            }

            if (kind == RobloxLookupKind::child) {
                return std::nullopt;
            }

            return result;
        }

        std::optional<Luau::WithPredicate<Luau::TypePackId>> handleOldSolver(Luau::TypeChecker &checker,
            const Luau::ScopePtr &scope, const Luau::AstExprCall &call,
            Luau::WithPredicate<Luau::TypePackId> arguments) override {
            auto result = lookup(*scope, call, Luau::first(arguments.type));

            if (!result) {
                if (kind != RobloxLookupKind::child && call.args.size &&
                    call.args.data[0]->is<Luau::AstExprConstantString>()) {
                    checker.reportError(Luau::TypeError{
                        call.args.data[0]->location,
                        Luau::GenericError{"Invalid Roblox class for this operation"},
                    });
                }

                return std::nullopt;
            }

            if (kind == RobloxLookupKind::class_type) {
                result = checker.currentModule->internalTypes->addType(Luau::UnionType{{*result, checker.nilType}});
            }

            return Luau::WithPredicate<Luau::TypePackId>{checker.currentModule->internalTypes->addTypePack({*result})};
        }

        bool infer(const Luau::MagicFunctionCallContext &context) override {
            auto result = lookup(*context.solver->rootScope, *context.callSite, Luau::first(context.arguments));

            if (!result) {
                if (kind != RobloxLookupKind::child && context.callSite->args.size &&
                    context.callSite->args.data[0]->is<Luau::AstExprConstantString>()) {
                    report_magic_error(
                        context, context.callSite->args.data[0]->location, "Invalid Roblox class for this operation");
                }

                return false;
            }

            if (kind == RobloxLookupKind::class_type) {
                result =
                    context.solver->arena->addType(Luau::UnionType{{*result, context.solver->builtinTypes->nilType}});
            }

            Luau::asMutable(context.result)
                ->ty.emplace<Luau::BoundTypePack>(context.solver->arena->addTypePack({*result}));

            return true;
        }
    };

    void attach_magic(Luau::TypeId type, const std::shared_ptr<Luau::MagicFunction> &magic) {
        type = Luau::follow(type);

        if (Luau::get<Luau::FunctionType>(type)) {
            Luau::attachMagicFunction(type, magic);
        } else if (auto intersection = Luau::get<Luau::IntersectionType>(type)) {
            for (auto part : intersection->parts) {
                attach_magic(part, magic);
            }
        }
    }

    void attach_magic(Luau::ExternType &type, const std::string &name, std::shared_ptr<Luau::MagicFunction> magic) {
        auto property = Luau::lookupExternTypeProp(&type, name);

        if (property && property->readTy) {
            attach_magic(*property->readTy, std::move(magic));
        }
    }

    Luau::TypeId external_type(
        Luau::Frontend &frontend, const std::string &name, std::optional<Luau::TypeId> parent = std::nullopt) {
        return frontend.globals.globalTypes.addType(Luau::ExternType{name, {}, parent, {}, {}, {}, "roblox", {}});
    }

    void prepare_roblox_types(Luau::Frontend &frontend, const instar::RobloxMetadata &metadata) {
        auto &arena = frontend.globals.globalTypes;
        auto scope = frontend.globals.globalScope;

        for (const auto name : {"Instance", "Enum", "EnumItem"}) {
            auto binding = scope->lookupType(name);

            if (!binding) {
                continue;
            }

            auto base = Luau::get<Luau::ExternType>(Luau::follow(binding->type));

            if (!base) {
                continue;
            }

            for (auto &entry : scope->exportedTypeBindings) {
                auto derived = Luau::getMutable<Luau::ExternType>(Luau::follow(entry.second.type));

                if (derived && Luau::isSubclass(derived, base)) {
                    derived->metatable = base->metatable;
                }
            }
        }

        for (const auto &name : metadata.enumerations) {
            if (auto binding = scope->lookupType("Enum" + name)) {
                scope->importedTypeBindings["Enum"][name] = *binding;
            }
        }

        for (const auto &record : metadata.classes) {
            auto binding = scope->lookupType(record.name);

            if (!binding) {
                continue;
            }

            if (auto type = Luau::getMutable<Luau::ExternType>(Luau::follow(binding->type))) {
                type->tags.push_back("RobloxClass");

                if (record.service) {
                    type->tags.push_back("RobloxService");
                }

                if (!record.creatable) {
                    type->tags.push_back("RobloxNotCreatable");
                }
            }
        }

        auto instance = scope->lookupType("Instance");

        if (instance) {
            auto type = Luau::getMutable<Luau::ExternType>(Luau::follow(instance->type));

            if (type) {
                attach_magic(*type, "IsA", std::make_shared<RobloxPredicate>());
                attach_magic(*type, "WaitForChild", std::make_shared<RobloxLookup>(RobloxLookupKind::child));
                attach_magic(*type, "FindFirstChild", std::make_shared<RobloxLookup>(RobloxLookupKind::child));

                for (const auto name : {
                         "FindFirstChildOfClass",
                         "FindFirstChildWhichIsA",
                         "FindFirstAncestorOfClass",
                         "FindFirstAncestorWhichIsA",
                     }) {
                    attach_magic(*type, name, std::make_shared<RobloxLookup>(RobloxLookupKind::class_type));
                }

                for (auto &binding : scope->exportedTypeBindings) {
                    auto derived = Luau::getMutable<Luau::ExternType>(Luau::follow(binding.second.type));

                    if (!derived) {
                        continue;
                    }

                    auto base = derived;

                    while (base != type && base && base->parent) {
                        base = Luau::getMutable<Luau::ExternType>(Luau::follow(*base->parent));
                    }

                    if (base != type) {
                        continue;
                    }

                    bool ancestor = false;

                    for (const auto &candidate : scope->exportedTypeBindings) {
                        auto child = Luau::get<Luau::ExternType>(Luau::follow(candidate.second.type));

                        if (child && child->parent &&
                            Luau::follow(*child->parent) == Luau::follow(binding.second.type)) {
                            ancestor = true;
                        }
                    }

                    auto property = Luau::lookupExternTypeProp(derived, "ClassName");

                    if (property && property->readTy) {
                        auto replacement = *property;

                        replacement.readTy =
                            ancestor ? frontend.builtinTypes->stringType
                                     : arena.addType(Luau::SingletonType{Luau::StringSingleton{binding.first}});

                        derived->props["ClassName"] = std::move(replacement);
                    }
                }
            }
        }

        if (auto binding = scope->lookup(Luau::Symbol{Luau::AstName("Instance")})) {
            if (auto table = Luau::get<Luau::TableType>(Luau::follow(*binding))) {
                auto constructor = table->props.find("new");

                if (constructor != table->props.end() && constructor->second.readTy) {
                    attach_magic(
                        *constructor->second.readTy, std::make_shared<RobloxLookup>(RobloxLookupKind::constructor));
                }
            }
        }

        if (auto model = scope->lookupType("DataModel")) {
            if (auto type = Luau::getMutable<Luau::ExternType>(Luau::follow(model->type))) {
                attach_magic(*type, "GetService", std::make_shared<RobloxLookup>(RobloxLookupKind::service));
            }
        }

        std::vector<Luau::TypeId> types;
        std::vector<std::string> names;

        for (const auto &record : metadata.nodes) {
            auto base = scope->lookupType(record.class_name);

            if (!base || !Luau::get<Luau::ExternType>(Luau::follow(base->type))) {
                throw std::runtime_error("missing Roblox instance type " + record.class_name);
            }

            types.push_back(external_type(frontend, record.class_name, base->type));
            Luau::getMutable<Luau::ExternType>(types.back())->userData = std::make_shared<RobloxInstances>();
            names.push_back(record.name);
        }

        for (size_t index = 0; index < types.size(); ++index) {
            auto type = Luau::getMutable<Luau::ExternType>(types[index]);
            auto inherited = Luau::lookupExternTypeProp(type, "Parent");

            if (inherited && inherited->readTy) {
                auto replacement = *inherited;

                replacement.readTy = metadata.nodes[index].parent ? types.at(*metadata.nodes[index].parent)
                                                                  : frontend.builtinTypes->nilType;

                type->props["Parent"] = std::move(replacement);
            }

            if (metadata.nodes[index].parent) {
                auto parent = types.at(*metadata.nodes[index].parent);
                auto parent_type = Luau::getMutable<Luau::ExternType>(parent);
                auto instances = std::static_pointer_cast<RobloxInstances>(parent_type->userData);
                instances->children[names[index]] = types[index];

                if (!Luau::lookupExternTypeProp(parent_type, names[index])) {
                    parent_type->props[names[index]] = Luau::Property::readonly(types[index]);
                }
            }
        }

        if (!types.empty() && Luau::get<Luau::ExternType>(types[0])->name == "DataModel") {
            scope->bindings[Luau::AstName("game")] = Luau::Binding{types[0]};
            scope->bindings[Luau::AstName("Game")] = Luau::Binding{types[0]};

            for (size_t index = 0; index < types.size(); ++index) {
                if (Luau::get<Luau::ExternType>(types[index])->name == "Workspace") {
                    scope->bindings[Luau::AstName("workspace")] = Luau::Binding{types[index]};
                    scope->bindings[Luau::AstName("Workspace")] = Luau::Binding{types[index]};
                    break;
                }
            }
        }

        for (auto type : types) {
            Luau::persist(type);
        }
    }

    struct Implementation {
        Implementation(std::vector<instar::Module> modules, instar::ModuleResolver resolver,
            std::vector<instar::Configuration> configurations)
            : modules(std::move(modules)), sources(this->modules, std::move(resolver)),
              configurations(std::move(configurations)) {}

        void reset() {
            sources.set_modules(modules);
            frontend.reset();
            checked = false;
            checked_modules.clear();
            builtins_loaded = false;
        }

        void ensure_frontend() {
            if (frontend) {
                return;
            }

            Luau::FrontendOptions options;
            options.retainFullTypeGraphs = true;
            options.runLintChecks = true;
            frontend = std::make_unique<Luau::Frontend>(Luau::SolverMode::New, &sources, &configurations, options);
        }

        void ensure_builtins() {
            ensure_frontend();

            if (!builtins_loaded) {
                Luau::registerBuiltinGlobals(*frontend, frontend->globals);
                builtins_loaded = true;
            }
        }

        void load_definitions(const std::vector<instar::Module> &definitions) {
            ensure_builtins();

            for (const auto &definition : definitions) {
                frontend->loadDefinitionFile(frontend->globals, frontend->globals.globalScope, definition.source,
                    normalize(definition.name), true);
            }
        }

        void prepare_roblox(const instar::RobloxMetadata &metadata) {
            ensure_builtins();
            prepare_roblox_types(*frontend, metadata);
        }

        void ensure_checked() {
            if (checked) {
                return;
            }

            initialize();
            ensure_builtins();

            for (const auto &module : modules) {
                frontend->queueModuleCheck(normalize(module.name));
            }

            checked_modules = frontend->checkQueuedModules();
            checked = true;
        }

        Luau::Module *module(const std::string &name) {
            ensure_checked();

            auto result = frontend->moduleResolver.getModule(normalize(name));

            return result ? result.get() : nullptr;
        }

        Luau::SourceModule *source(const std::string &name) {
            ensure_checked();

            return frontend->getSourceModule(normalize(name));
        }

        std::vector<instar::Module> modules;
        Sources sources;
        Configurations configurations;
        std::unique_ptr<Luau::Frontend> frontend;
        std::vector<std::string> checked_modules;
        bool checked = false;
        bool builtins_loaded = false;
    };

    std::optional<Luau::TypeId> type_at(
        Luau::Module &module, const Luau::SourceModule &source, instar::Position position) {
        return Luau::findTypeAtPosition(module, source, {position.line, position.column});
    }

    std::string type_description(Luau::TypeId type) {
        return Luau::toString(Luau::follow(type));
    }

    std::optional<instar::Destination> destination(Implementation &implementation, Luau::Module &module,
        const Luau::SourceModule &source, const std::string &path, instar::Position position, bool type_only) {
        auto native_position = Luau::Position{position.line, position.column};
        auto ancestry = Luau::findAstAncestryOfPosition(source, native_position, true);

        if (ancestry.empty()) {
            return std::nullopt;
        }

        auto expression_or_local = Luau::findExprOrLocalAtPosition(source, native_position);

        if (auto local = expression_or_local.getLocal()) {
            return instar::Destination{path, range(local->location), local->name.value};
        }

        if (auto binding = Luau::findBindingAtPosition(module, source, native_position)) {
            auto name = expression_or_local.getName();

            return instar::Destination{
                path,
                range(binding->location),
                name && name->value ? std::string(name->value) : std::string{},
            };
        }

        auto *node = ancestry.back();

        if (!type_only) {
            if (auto index = node->as<Luau::AstExprIndexName>()) {
                if (auto owner = module.astTypes.find(index->expr)) {
                    if (auto table = Luau::get<Luau::TableType>(Luau::follow(*owner))) {
                        auto property = table->props.find(index->index.value);

                        if (property != table->props.end()) {
                            auto property_location =
                                property->second.location ? property->second.location : property->second.typeLocation;

                            if (property_location) {
                                return instar::Destination{
                                    table->definitionModuleName.empty() ? path : table->definitionModuleName,
                                    range(*property_location),
                                    index->index.value,
                                };
                            }
                        }
                    }
                }
            }
        }

        if (auto type = type_at(module, source, position)) {
            auto followed = Luau::follow(*type);

            if (auto function = Luau::get<Luau::FunctionType>(followed)) {
                if (function->definition) {
                    auto definition = *function->definition;
                    auto definition_path = definition.definitionModuleName.value_or(path);
                    auto definition_location = definition.originalNameLocation;

                    if (definition_location.begin == definition_location.end) {
                        definition_location = definition.definitionLocation;
                    }

                    return instar::Destination{definition_path, range(definition_location), {}};
                }
            }

            if (type_only) {
                if (auto table = Luau::get<Luau::TableType>(followed)) {
                    return instar::Destination{
                        table->definitionModuleName.empty() ? path : table->definitionModuleName,
                        range(table->definitionLocation),
                        table->name.value_or(std::string{}),
                    };
                }

                if (auto external = Luau::get<Luau::ExternType>(followed)) {
                    if (external->definitionLocation) {
                        return instar::Destination{
                            external->definitionModuleName,
                            range(*external->definitionLocation),
                            external->name,
                        };
                    }
                }
            }
        }

        return std::nullopt;
    }

    struct ImplementationVisitor final : Luau::AstVisitor {
        const std::string &path;
        const instar::Destination &target;
        const Luau::Module &module;
        std::vector<instar::Destination> results;

        ImplementationVisitor(const std::string &path, const instar::Destination &target, const Luau::Module &module)
            : path(path), target(target), module(module) {}

        bool matches(const Luau::Location &location) const {
            return target.path == path && target.range.begin.line == location.begin.line &&
                   target.range.begin.column == location.begin.column && target.range.end.line == location.end.line &&
                   target.range.end.column == location.end.column;
        }

        bool visit(Luau::AstExprFunction *function) override {
            if (auto type = module.astTypes.find(function)) {
                if (auto signature = Luau::get<Luau::FunctionType>(Luau::follow(*type));
                    signature && signature->definition) {
                    auto definition = *signature->definition;
                    auto name_location = definition.originalNameLocation;

                    if (name_location.begin == name_location.end) {
                        name_location = definition.definitionLocation;
                    }

                    if (matches(name_location)) {
                        results.push_back({path, range(function->location), {}});
                    }
                }
            }

            return true;
        }

        bool visit(Luau::AstExprTable *table) override {
            if (auto type = module.astTypes.find(table)) {
                if (auto contract = Luau::get<Luau::TableType>(Luau::follow(*type));
                    contract && matches(contract->definitionLocation)) {
                    results.push_back({path, range(table->location), contract->name.value_or(std::string{})});
                }
            }

            return true;
        }
    };

    struct ReferenceVisitor final : Luau::AstVisitor {
        const std::string &path;
        const instar::Destination &target;
        const Luau::Module &module;
        std::vector<instar::Destination> results;

        ReferenceVisitor(const std::string &path, const instar::Destination &target, const Luau::Module &module)
            : path(path), target(target), module(module) {}

        bool matches(const Luau::Location &location) const {
            return target.path == path && target.range.begin.line == location.begin.line &&
                   target.range.begin.column == location.begin.column && target.range.end.line == location.end.line &&
                   target.range.end.column == location.end.column;
        }

        bool visit(Luau::AstExprLocal *local) override {
            if (matches(local->local->location)) {
                results.push_back({path, range(local->location), local->local->name.value});
            }

            return true;
        }

        bool visit(Luau::AstExprIndexName *index) override {
            if (auto owner = module.astTypes.find(index->expr)) {
                if (auto table = Luau::get<Luau::TableType>(Luau::follow(*owner))) {
                    auto property = table->props.find(index->index.value);

                    if (property != table->props.end()) {
                        auto property_location =
                            property->second.location ? property->second.location : property->second.typeLocation;

                        if (property_location && matches(*property_location)) {
                            results.push_back({path, range(index->indexLocation), index->index.value});
                        }
                    }
                }
            }

            return true;
        }
    };

    struct CallVisitor final : Luau::AstVisitor {
        Implementation &implementation;
        const std::string &path;
        Luau::Module &module;
        const Luau::SourceModule &source;
        std::vector<instar::Call> results;

        CallVisitor(Implementation &implementation, const std::string &path, Luau::Module &module,
            const Luau::SourceModule &source)
            : implementation(implementation), path(path), module(module), source(source) {}

        bool visit(Luau::AstExprCall *call) override {
            auto position = call->func->location.end;

            if (!position.column) {
                return true;
            }

            --position.column;
            auto target = destination(implementation, module, source, path, {position.line, position.column}, false);

            if (!target) {
                return true;
            }

            std::optional<instar::Range> container;
            auto ancestry = Luau::findAstAncestryOfPosition(source, call->location.begin);

            for (auto iterator = ancestry.rbegin(); iterator != ancestry.rend(); ++iterator) {
                if (auto function = (*iterator)->as<Luau::AstExprFunction>()) {
                    container = range(function->location);
                    break;
                }
            }

            results.push_back({*target, range(call->func->location), container});

            return true;
        }
    };

} // namespace

namespace instar {

    struct Engine::Implementation : ::Implementation {
        using ::Implementation::Implementation;
    };

    Engine::Engine(std::vector<Module> modules, ModuleResolver resolver, std::vector<Configuration> configurations)
        : implementation(
              std::make_unique<Implementation>(std::move(modules), std::move(resolver), std::move(configurations))) {}

    Engine::~Engine() = default;
    Engine::Engine(Engine &&) noexcept = default;
    Engine &Engine::operator=(Engine &&) noexcept = default;

    void Engine::update(std::vector<Module> modules, std::vector<Configuration> configurations) {
        implementation->modules = std::move(modules);
        implementation->configurations.set_configurations(std::move(configurations));
        implementation->reset();
    }

    void Engine::load_definitions(std::vector<Module> definitions) {
        implementation->load_definitions(definitions);
    }

    void Engine::prepare_roblox(const RobloxMetadata &metadata) {
        implementation->prepare_roblox(metadata);
    }

    std::vector<Diagnostic> Engine::check() {
        implementation->ensure_checked();
        std::vector<Diagnostic> diagnostics;

        for (const auto &name : implementation->checked_modules) {
            auto result = implementation->frontend->getCheckResult(name, false);

            if (!result) {
                diagnostics.push_back({name, "native analysis result unavailable", {}, true});
                continue;
            }

            for (const auto &error : result->errors) {
                std::string message;

                if (const auto *syntax = Luau::get_if<Luau::SyntaxError>(&error.data)) {
                    message = "SyntaxError: " + syntax->message;
                } else {
                    message =
                        "TypeError: " + Luau::toString(error, Luau::TypeErrorToStringOptions{&implementation->sources});
                }

                diagnostics.push_back({
                    error.moduleName.empty() ? name : error.moduleName,
                    std::move(message),
                    range(error.location),
                    true,
                });
            }

            for (const auto &warning : result->lintResult.errors) {
                diagnostics.push_back({name, warning.text, range(warning.location), true});
            }

            for (const auto &warning : result->lintResult.warnings) {
                diagnostics.push_back({name, warning.text, range(warning.location), false});
            }
        }

        return diagnostics;
    }

    std::optional<TypeInformation> Engine::type_at(const std::string &path, Position position) {
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return std::nullopt;
        }

        auto type = ::type_at(*module, *source, position);

        return type ? std::optional<TypeInformation>{{type_description(*type)}} : std::nullopt;
    }

    std::optional<TypeInformation> Engine::hover(const std::string &path, Position position) {
        return type_at(path, position);
    }

    std::vector<Completion> Engine::complete(const std::string &path, Position position) {
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);
        std::vector<Completion> result;

        if (!module || !source) {
            return result;
        }

        auto completions =
            Luau::autocomplete(*implementation->frontend, normalize(path), {position.line, position.column}, {});

        for (const auto &[label, entry] : completions.entryMap) {
            Completion completion{
                label,
                entry.type ? type_description(*entry.type) : std::string{},
                entry.documentationSymbol.value_or(std::string{}),
                entry.insertText.value_or(std::string{}),
                static_cast<unsigned>(entry.kind),
                entry.deprecated,
            };

            result.push_back(std::move(completion));
        }

        std::sort(result.begin(), result.end(),
            [](const Completion &left, const Completion &right) { return left.label < right.label; });

        return result;
    }

    std::optional<Signature> Engine::signature(const std::string &path, Position position) {
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return std::nullopt;
        }

        auto ancestry = Luau::findAstAncestryOfPosition(*source, {position.line, position.column}, true);

        for (auto iterator = ancestry.rbegin(); iterator != ancestry.rend(); ++iterator) {
            auto call = (*iterator)->as<Luau::AstExprCall>();

            if (!call) {
                continue;
            }

            auto type = module->astOriginalCallTypes.find(call->func);

            if (!type) {
                type = module->astTypes.find(call->func);
            }

            if (!type) {
                return std::nullopt;
            }

            auto function = Luau::get<Luau::FunctionType>(Luau::follow(*type));

            if (!function) {
                return std::nullopt;
            }

            unsigned active = 0;

            for (size_t index = 0; index < call->args.size; ++index) {
                if (call->args.data[index]->location.contains({position.line, position.column})) {
                    active = unsigned(index);
                    break;
                }

                if (call->args.data[index]->location.end < Luau::Position{position.line, position.column}) {
                    active = unsigned(index + 1);
                }
            }

            std::vector<std::string> parameters;

            for (const auto &argument : function->argNames) {
                parameters.push_back(argument ? argument->name : std::string{});
            }

            return Signature{type_description(*type), std::move(parameters), active};
        }

        return std::nullopt;
    }

    std::optional<Destination> Engine::definition(const std::string &path, Position position) {
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return std::nullopt;
        }

        return ::destination(*implementation, *module, *source, normalize(path), position, false);
    }

    std::optional<Destination> Engine::type_definition(const std::string &path, Position position) {
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return std::nullopt;
        }

        return ::destination(*implementation, *module, *source, normalize(path), position, true);
    }

    std::vector<Destination> Engine::implementations(const std::string &path, Position position) {
        std::vector<Destination> result;
        auto target = definition(path, position);

        if (!target) {
            return result;
        }

        implementation->ensure_checked();

        for (const auto &name : implementation->checked_modules) {
            auto *module = implementation->module(name);
            auto *source = implementation->source(name);

            if (!module || !source) {
                continue;
            }

            ImplementationVisitor visitor(name, *target, *module);
            source->root->visit(&visitor);
            result.insert(result.end(), visitor.results.begin(), visitor.results.end());
        }

        return result;
    }

    std::vector<Destination> Engine::references(const std::string &path, Position position) {
        std::vector<Destination> result;
        auto target = definition(path, position);

        if (!target) {
            return result;
        }

        implementation->ensure_checked();

        for (const auto &name : implementation->checked_modules) {
            auto *module = implementation->module(name);
            auto *source = implementation->source(name);

            if (!module || !source) {
                continue;
            }

            ReferenceVisitor visitor(name, *target, *module);
            source->root->visit(&visitor);
            result.insert(result.end(), visitor.results.begin(), visitor.results.end());
        }

        return result;
    }

    std::vector<Annotation> Engine::annotations(const std::string &path) {
        std::vector<Annotation> result;
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return result;
        }

        struct Visitor final : Luau::AstVisitor {
            const Luau::Module &module;
            std::vector<Annotation> &result;
            std::string path;

            Visitor(const Luau::Module &module, std::vector<Annotation> &result, std::string path)
                : module(module), result(result), path(std::move(path)) {}

            void local(Luau::AstLocal *local) {
                if (local->annotation) {
                    return;
                }

                auto scope = Luau::findScopeAtPosition(module, local->location.begin);

                if (scope) {
                    if (auto type = scope->lookup(local)) {
                        result.push_back({path, {local->location.end.line, local->location.end.column},
                            ": " + type_description(*type)});
                    }
                }
            }

            bool visit(Luau::AstStatLocal *statement) override {
                for (auto *local : statement->vars) {
                    this->local(local);
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *function) override {
                for (auto *argument : function->args) {
                    local(argument);
                }

                if (!function->returnAnnotation && function->argLocation) {
                    if (auto type = module.astTypes.find(function)) {
                        if (auto signature = Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                            result.push_back({
                                path,
                                {function->argLocation->end.line, function->argLocation->end.column},
                                ": " + Luau::toString(signature->retTypes),
                            });
                        }
                    }
                }

                return true;
            }
        } visitor(*module, result, normalize(path));

        source->root->visit(&visitor);

        return result;
    }

    std::vector<Call> Engine::calls(const std::string &path) {
        std::vector<Call> result;
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return result;
        }

        CallVisitor visitor(*implementation, normalize(path), *module, *source);
        source->root->visit(&visitor);

        return visitor.results;
    }

    AliasResult parse_aliases(const std::string &source, bool executable) {
        initialize();

        Luau::Config configuration;
        Luau::ConfigOptions options;
        options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};

        auto error = executable ? Luau::extractLuauConfig(source, configuration, options.aliasOptions, {})
                                : Luau::parseConfig(source, configuration, options);

        AliasResult result;

        if (error) {
            result.error = std::move(*error);

            return result;
        }

        for (const auto &entry : configuration.aliases) {
            result.aliases.push_back({entry.second.originalCase, entry.second.value});
        }

        return result;
    }

    std::vector<Diagnostic> analyze(const std::vector<Module> &modules) {
        return Engine(modules).check();
    }

} // namespace instar
