#include "engine.hpp"

#include "Luau/Ast.h"
#include "Luau/AstJsonEncoder.h"
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
#include <regex>
#include <stdexcept>
#include <string>
#include <unordered_set>
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
        Sources(
            const std::vector<instar::Module> &modules, instar::ModuleReader reader, instar::ModuleResolver resolver)
            : reader(std::move(reader)), resolver(std::move(resolver)) {
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
                if (!reader) {
                    return std::nullopt;
                }

                auto source = reader(name);

                if (!source) {
                    return std::nullopt;
                }

                return Luau::SourceCode{std::move(*source), Luau::SourceCode::Module};
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
        instar::ModuleReader reader;
        instar::ModuleResolver resolver;
    };

    bool is_configuration(const std::string &path) {
        auto name = path.substr(path.find_last_of("/\\") + 1);

        return name == ".config.luau" || name == "config.luau";
    }

    class Configurations final : public Luau::ConfigResolver {
        public:
        explicit Configurations(std::vector<instar::Configuration> configurations, std::optional<instar::Mode> mode)
            : configurations(std::move(configurations)), mode(mode) {}

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

            if (mode) {
                iterator->second.mode = mode.value() == instar::Mode::Strict    ? Luau::Mode::Strict
                                        : mode.value() == instar::Mode::NoCheck ? Luau::Mode::NoCheck
                                                                                : Luau::Mode::Nonstrict;
            }

            return iterator->second;
        }

        private:
        std::vector<instar::Configuration> configurations;
        std::optional<instar::Mode> mode;
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
        Implementation(std::vector<instar::Module> modules, instar::ModuleReader reader,
            instar::ModuleResolver resolver, std::vector<instar::Configuration> configurations, instar::Solver solver,
            std::optional<instar::Mode> mode)
            : modules(std::move(modules)), sources(this->modules, std::move(reader), std::move(resolver)),
              configurations(std::move(configurations), mode), solver(solver) {}

        void reset() {
            sources.set_modules(modules);
            frontend.reset();
            checked = false;
            checked_modules.clear();
            target_scopes.clear();
            builtins_loaded = false;
        }

        void ensure_frontend() {
            if (frontend) {
                return;
            }

            Luau::FrontendOptions options;
            options.retainFullTypeGraphs = true;
            options.runLintChecks = true;

            frontend = std::make_unique<Luau::Frontend>(
                solver == instar::Solver::Old ? Luau::SolverMode::Old : Luau::SolverMode::New, &sources,
                &configurations, options);
        }

        void ensure_builtins() {
            ensure_frontend();

            if (!builtins_loaded) {
                Luau::registerBuiltinGlobals(*frontend, frontend->globals);
                builtins_loaded = true;
            }
        }

        void load_definitions(
            std::vector<instar::Module> definitions, std::optional<std::vector<std::string>> target_paths) {
            ensure_builtins();
            target_scopes.clear();
            frontend->prepareModuleScope = {};

            if (!target_paths) {
                for (const auto &definition : definitions) {
                    frontend->loadDefinitionFile(frontend->globals, frontend->globals.globalScope, definition.source,
                        normalize(definition.name), true);
                }

                return;
            }

            for (const auto &target : *target_paths) {
                auto scope = std::make_shared<Luau::Scope>(frontend->globals.globalScope);

                for (const auto &definition : definitions) {
                    auto result = frontend->loadDefinitionFile(
                        frontend->globals, scope, definition.source, normalize(definition.name), false);

                    if (!result.success) {
                        throw std::runtime_error("definition source failed: " + definition.name);
                    }
                }

                target_scopes.emplace(normalize(target), std::move(scope));
            }

            frontend->prepareModuleScope = [this](const Luau::ModuleName &name, const Luau::ScopePtr &scope, bool) {
                auto target = target_scopes.find(normalize(name));

                if (target == target_scopes.end()) {
                    return;
                }

                for (const auto &binding : target->second->exportedTypeBindings) {
                    scope->exportedTypeBindings[binding.first] = binding.second;
                }

                if (auto configuration = target->second->lookupType("Config")) {
                    scope->returnType = frontend->globals.globalTypes.addTypePack({configuration->type});
                }
            };
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
        instar::Solver solver;
        std::unique_ptr<Luau::Frontend> frontend;
        std::vector<std::string> checked_modules;
        std::map<std::string, std::shared_ptr<Luau::Scope>> target_scopes;
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
        std::string path;
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

    struct SymbolVisitor final : Luau::AstVisitor {
        Implementation &implementation;
        Luau::Module &module;
        const Luau::SourceModule &source;
        std::string path;
        bool tokens;
        bool links;
        std::unordered_set<Luau::AstLocal *> parameters;
        std::vector<instar::Symbol> results;

        SymbolVisitor(Implementation &implementation, Luau::Module &module, const Luau::SourceModule &source,
            std::string path, bool tokens, bool links)
            : implementation(implementation), module(module), source(source), path(std::move(path)), tokens(tokens),
              links(links) {}

        void entry(const std::string &name, Luau::Location selection, Luau::Location location, unsigned kind,
            bool declaration, unsigned modifiers = 0) {

            if (links && kind != 3) {
                return;
            }

            if (!tokens && !links && !declaration) {
                return;
            }

            results.push_back(
                {path, path, range(tokens ? selection : location), range(selection), kind, declaration, modifiers});
        }

        void local(Luau::AstLocal *local, Luau::Location location) {
            unsigned kind = 13;

            if (auto scope = Luau::findScopeAtPosition(module, local->location.begin)) {
                if (auto type = scope->lookup(local); type && Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                    kind = 12;
                }
            }

            entry(local->name.value, local->location, location, tokens && parameters.count(local) ? 27 : kind, true,
                local->isConst ? 2 : 0);
        }

        bool visit(Luau::AstStatLocal *statement) override {
            for (auto *local : statement->vars) {
                this->local(local, statement->location);
            }

            return true;
        }

        bool visit(Luau::AstStatLocalFunction *statement) override {
            local(statement->name, statement->location);

            return true;
        }

        bool visit(Luau::AstStatFunction *statement) override {
            if (auto *global = statement->name->as<Luau::AstExprGlobal>()) {
                entry(global->name.value, global->location, statement->location, 12, true);
            } else if (auto *index = statement->name->as<Luau::AstExprIndexName>()) {
                entry(index->index.value, index->indexLocation, statement->location,
                    tokens && index->op == ':' ? 6 : 12, true);

                index->expr->visit(this);
            } else {
                statement->name->visit(this);
            }

            statement->func->visit(this);

            return false;
        }

        bool visit(Luau::AstExprFunction *function) override {
            if (function->self) {
                parameters.insert(function->self);
            }

            for (auto *argument : function->args) {
                parameters.insert(argument);
                local(argument, argument->location);
            }

            return true;
        }

        bool visit(Luau::AstStatFor *statement) override {
            local(statement->var, statement->var->location);

            return true;
        }

        bool visit(Luau::AstStatForIn *statement) override {
            for (auto *local : statement->vars) {
                this->local(local, local->location);
            }

            return true;
        }

        bool visit(Luau::AstExprTable *expression) override {
            for (const auto &item : expression->items) {
                if (item.kind == Luau::AstExprTable::Item::Kind::Record) {
                    if (auto *key = item.key->as<Luau::AstExprConstantString>()) {
                        entry(std::string(key->value.data, key->value.size), key->location,
                            Luau::Location(key->location.begin, item.value->location.end),
                            item.value->is<Luau::AstExprFunction>() ? 12 : 7, true);
                    }
                }
            }

            return true;
        }

        bool visit(Luau::AstType *) override { return true; }

        bool visit(Luau::AstTypePack *) override { return true; }

        bool visit(Luau::AstExprCall *call) override {
            if (!tokens && !links || call->args.size != 1) {
                return true;
            }

            if (auto *global = call->func->as<Luau::AstExprGlobal>(); global && global->name == "require") {
                auto *argument = call->args.data[0];

                if (links || argument->is<Luau::AstExprConstantString>()) {
                    auto trace = implementation.frontend->requireTrace.find(path);

                    if (trace != implementation.frontend->requireTrace.end()) {
                        if (auto resolved = trace->second.exprs.find(argument);
                            resolved && implementation.frontend->getSourceModule(resolved->name)) {
                            entry(resolved->name, argument->location, argument->location, 3, false);
                        }
                    }
                }
            }

            return true;
        }

        bool visit(Luau::AstGenericType *generic) override {
            if (tokens) {
                entry(generic->name.value, generic->location, generic->location, 28, true);
            }

            return true;
        }

        bool visit(Luau::AstGenericTypePack *generic) override {
            if (tokens) {
                entry(generic->name.value, generic->location, generic->location, 28, true);
            }

            return true;
        }

        bool visit(Luau::AstTypeReference *reference) override {
            if (reference->prefix && reference->prefixLocation) {
                entry(reference->prefix->value, *reference->prefixLocation, *reference->prefixLocation, 13, false);
            }

            unsigned kind = 26;

            if (tokens) {
                if (auto type = module.astResolvedTypes.find(reference)) {
                    auto followed = Luau::follow(*type);

                    if (Luau::get<Luau::GenericType>(followed)) {
                        kind = 28;
                    } else if (Luau::get<Luau::ExternType>(followed)) {
                        kind = 5;
                    }
                }
            }

            entry(reference->name.value, reference->nameLocation, reference->nameLocation, kind, false);

            return true;
        }

        bool visit(Luau::AstStatTypeAlias *statement) override {
            entry(statement->name.value, statement->nameLocation, statement->location, 26, true);

            return true;
        }

        bool visit(Luau::AstExprLocal *expression) override {
            unsigned kind = 13;

            if (tokens) {
                if (parameters.count(expression->local)) {
                    kind = 27;
                } else if (auto type = module.astTypes.find(expression);
                    type && Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                    kind = 12;
                }
            }

            entry(expression->local->name.value, expression->location, expression->location, kind, false,
                expression->local->isConst ? 2 : 0);

            return true;
        }

        bool visit(Luau::AstExprGlobal *expression) override {
            entry(expression->name.value, expression->location, expression->location, 13, false);

            return true;
        }

        bool visit(Luau::AstExprIndexName *expression) override {
            unsigned kind = 7;

            if (tokens) {
                if (auto type = module.astTypes.find(expression);
                    type && Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                    kind = expression->op == ':' ? 6 : 12;
                }
            }

            entry(expression->index.value, expression->indexLocation, expression->indexLocation, kind, false);

            return true;
        }
    };

    struct ImportVisitor {
        Implementation &implementation;
        std::string path;
        Luau::Position position;
        std::vector<instar::Import> results;

        void visit(Luau::AstStat *statement) {
            auto local = statement->as<Luau::AstStatLocal>();

            if (!local || local->vars.size != 1 || local->values.size != 1 || !(local->location.end < position)) {
                return;
            }

            auto call = local->values.data[0]->as<Luau::AstExprCall>();

            if (!call || call->args.size != 1) {
                return;
            }

            std::string label;
            std::string target;

            if (auto global = call->func->as<Luau::AstExprGlobal>(); global && global->name == "require") {
                auto trace = implementation.frontend->requireTrace.find(path);

                if (trace != implementation.frontend->requireTrace.end()) {
                    if (auto resolved = trace->second.exprs.find(call->args.data[0])) {
                        label = "module";
                        target = resolved->name;
                    }
                }
            } else if (auto member = call->func->as<Luau::AstExprIndexName>();
                member && member->index == "GetService") {
                auto global = member->expr->as<Luau::AstExprGlobal>();
                auto service = call->args.data[0]->as<Luau::AstExprConstantString>();

                if (global && global->name == "game" && service) {
                    label = "service";
                    target.assign(service->value.data, service->value.size);
                }
            }

            if (!label.empty()) {
                results.push_back({local->vars.data[0]->name.value, label, target, range(local->location)});
            }
        }
    };

} // namespace

namespace instar {

    struct Engine::Implementation : ::Implementation {
        using ::Implementation::Implementation;
    };

    Engine::Engine(std::vector<Module> modules, ModuleReader reader, ModuleResolver resolver,
        std::vector<Configuration> configurations, Solver solver, std::optional<Mode> mode)
        : implementation(std::make_unique<Implementation>(
              std::move(modules), std::move(reader), std::move(resolver), std::move(configurations), solver, mode)) {}

    Engine::~Engine() = default;
    Engine::Engine(Engine &&) noexcept = default;
    Engine &Engine::operator=(Engine &&) noexcept = default;

    void Engine::update(std::vector<Module> modules, std::vector<Configuration> configurations) {
        implementation->modules = std::move(modules);
        implementation->configurations.set_configurations(std::move(configurations));
        implementation->reset();
    }

    void Engine::load_definitions(
        std::vector<Module> definitions, std::optional<std::vector<std::string>> target_paths) {
        implementation->load_definitions(std::move(definitions), std::move(target_paths));
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

    std::optional<Syntax> Engine::syntax(const std::string &path) {
        auto *source = implementation->source(path);

        if (!source || !source->root) {
            return std::nullopt;
        }

        std::vector<std::string> parameters;

        for (const auto &[symbol, binding] : implementation->frontend->globals.globalScope->bindings) {
            if (symbol.global.value) {
                parameters.emplace_back(symbol.global.value);
            }
        }

        return Syntax{Luau::toJson(source->root, source->commentLocations), std::move(parameters)};
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

    std::vector<Extract> Engine::extract(const std::string &path, Position position) {
        std::vector<Extract> result;
        auto *source = implementation->source(path);

        if (!source) {
            return result;
        }

        auto native_position = Luau::Position{position.line, position.column};

        for (auto *statement : source->root->body) {
            Luau::AstExpr *expression = nullptr;

            if (auto *local = statement->as<Luau::AstStatLocal>();
                local && local->vars.size == 1 && local->values.size == 1) {
                expression = local->values.data[0];
            } else if (auto *returned = statement->as<Luau::AstStatReturn>();
                returned && returned->list.size == 1 && !returned->list.data[0]->is<Luau::AstExprCall>() &&
                !returned->list.data[0]->is<Luau::AstExprVarargs>()) {
                expression = returned->list.data[0];
            }

            if (expression && expression->location.contains(native_position)) {
                result.push_back({range(expression->location), range(statement->location)});
            }
        }

        return result;
    }

    std::vector<Symbol> Engine::tokens(const std::string &path) {
        std::vector<Symbol> result;
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return result;
        }

        SymbolVisitor visitor(*implementation, *module, *source, normalize(path), true, false);
        source->root->visit(&visitor);

        return visitor.results;
    }

    std::vector<Symbol> Engine::index(const std::string &path) {
        std::vector<Symbol> result;
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return result;
        }

        SymbolVisitor symbols(*implementation, *module, *source, normalize(path), false, false);
        source->root->visit(&symbols);
        result.insert(result.end(), symbols.results.begin(), symbols.results.end());

        SymbolVisitor links(*implementation, *module, *source, normalize(path), false, true);
        source->root->visit(&links);
        result.insert(result.end(), links.results.begin(), links.results.end());

        return result;
    }

    std::vector<Import> Engine::imports(const std::string &path, Position position) {
        std::vector<Import> result;
        auto *source = implementation->source(path);

        if (!source) {
            return result;
        }

        ImportVisitor visitor{*implementation, normalize(path), {position.line, position.column}, {}};

        for (auto *statement : source->root->body) {
            visitor.visit(statement);
        }

        return visitor.results;
    }

    std::optional<Scope> Engine::scope(const std::string &path, Position position) {
        auto *module = implementation->module(path);
        auto *source = implementation->source(path);

        if (!module || !source) {
            return std::nullopt;
        }

        Scope result;
        auto native_position = Luau::Position{position.line, position.column};
        auto value = Luau::findExprOrLocalAtPosition(*source, native_position);

        if (value.getLocal() || (value.getExpr() && value.getExpr()->is<Luau::AstExprLocal>())) {
            result.kind = 13;
        }

        auto ancestry = Luau::findAstAncestryOfPosition(*source, native_position, true);

        if (!ancestry.empty()) {
            if (auto *member = ancestry.back()->as<Luau::AstExprIndexName>();
                member && member->indexLocation.contains(native_position)) {
                if (auto type = ::type_at(*module, *source, position);
                    type && !Luau::get<Luau::TableType>(Luau::follow(*type))) {
                    result.name = member->index.value;
                }
            }
        }

        return result;
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

} // namespace instar

namespace {

    std::string native_text(NativeBytes value) {
        if (value.size && !value.data) {
            throw std::runtime_error("native byte range is invalid");
        }

        return value.size ? std::string(value.data, value.size) : std::string{};
    }

    NativeBytes native_bytes(const std::string &value) {
        return {value.data(), value.size()};
    }

    NativeRange native_range(const instar::Range &value) {
        return {
            {value.begin.line, value.begin.column},
            {value.end.line, value.end.column},
        };
    }

    std::vector<instar::Module> native_modules(const NativeModule *values, std::size_t count) {
        if (count && !values) {
            throw std::runtime_error("native module list is invalid");
        }

        std::vector<instar::Module> modules;

        for (std::size_t index = 0; index < count; ++index) {
            modules.push_back({native_text(values[index].name), native_text(values[index].source)});
        }

        return modules;
    }

    instar::ModuleReader native_reader(void *context, NativeModuleReader reader) {
        if (!reader) {
            return {};
        }

        return [context, reader](const std::string &name) {
            auto result = reader(context, native_bytes(name));

            if (!result.data) {
                return std::optional<std::string>{};
            }

            return std::optional<std::string>{native_text(result)};
        };
    }

    instar::ModuleResolver native_resolver(void *context, NativeModuleResolver resolver) {
        if (!resolver) {
            return {};
        }

        return [context, resolver](const std::string &from, instar::Range range, std::string_view specifier) {
            auto result =
                resolver(context, native_bytes(from), native_range(range), {specifier.data(), specifier.size()});

            if (!result.data) {
                return std::optional<std::string>{};
            }

            return std::optional<std::string>{native_text(result)};
        };
    }

    void report_failure(void *context, NativeFailure failure, const std::string &message) {
        if (failure) {
            failure(context, native_bytes(message));
        }
    }

    template <typename Operation> void run(void *context, NativeFailure failure, Operation operation) {
        try {
            operation();
        } catch (const std::exception &error) {
            report_failure(context, failure, error.what());
        } catch (...) {
            report_failure(context, failure, "native engine failure");
        }
    }

} // namespace

extern "C" {

    void *instar_engine_create(const NativeModule *modules, std::size_t count, void *context, NativeModuleReader reader,
        NativeModuleResolver resolver, const NativeConfiguration *configurations, std::size_t configuration_count,
        int mode_value, int old_solver) {

        try {
            if (old_solver != 0 && old_solver != 1) {
                throw std::runtime_error("native solver selection is invalid");
            }

            std::optional<instar::Mode> mode;

            switch (mode_value) {
            case NativeModeConfigured:
                break;

            case NativeModeStrict:
                mode = instar::Mode::Strict;
                break;

            case NativeModeNonstrict:
                mode = instar::Mode::Nonstrict;
                break;

            case NativeModeNoCheck:
                mode = instar::Mode::NoCheck;
                break;

            default:
                throw std::runtime_error("native type-checking mode is invalid");
            }

            if (configuration_count && !configurations) {
                throw std::runtime_error("native configuration list is invalid");
            }

            std::vector<instar::Configuration> native_configurations;

            for (std::size_t index = 0; index < configuration_count; ++index) {
                native_configurations.push_back({
                    native_text(configurations[index].module),
                    native_text(configurations[index].path),
                    native_text(configurations[index].source),
                });
            }

            return new instar::Engine(native_modules(modules, count), native_reader(context, reader),
                native_resolver(context, resolver), std::move(native_configurations),
                old_solver ? instar::Solver::Old : instar::Solver::New, mode);
        } catch (...) {
            return nullptr;
        }
    }

    void instar_engine_destroy(void *engine) {
        delete static_cast<instar::Engine *>(engine);
    }

    int instar_engine_load_definitions(void *engine, const NativeModule *definitions, std::size_t definition_count,
        const NativeBytes *target_paths, std::size_t target_count, int all_targets, void *context,
        NativeFailure failure) {

        if (!engine) {
            report_failure(context, failure, "native engine is unavailable");

            return 0;
        }

        int success = 0;

        run(context, failure, [&] {
            std::optional<std::vector<std::string>> targets;

            if (!all_targets) {
                if (target_count && !target_paths) {
                    throw std::runtime_error("native target list is invalid");
                }

                std::vector<std::string> values;

                for (std::size_t index = 0; index < target_count; ++index) {
                    values.push_back(native_text(target_paths[index]));
                }

                targets = std::move(values);
            }

            static_cast<instar::Engine *>(engine)->load_definitions(
                native_modules(definitions, definition_count), std::move(targets));
            success = 1;
        });

        return success;
    }

    int instar_engine_prepare_roblox(void *engine, const NativeBytes *enumerations, std::size_t enumeration_count,
        const NativeRobloxClass *classes, std::size_t class_count, const NativeRobloxNode *nodes,
        std::size_t node_count, void *context, NativeFailure failure) {

        if (!engine) {
            report_failure(context, failure, "native engine is unavailable");

            return 0;
        }

        int success = 0;

        run(context, failure, [&] {
            if ((enumeration_count && !enumerations) || (class_count && !classes) || (node_count && !nodes)) {
                throw std::runtime_error("native Roblox metadata is invalid");
            }

            instar::RobloxMetadata metadata;

            for (std::size_t index = 0; index < enumeration_count; ++index) {
                metadata.enumerations.push_back(native_text(enumerations[index]));
            }

            for (std::size_t index = 0; index < class_count; ++index) {
                metadata.classes.push_back({
                    native_text(classes[index].name),
                    classes[index].service != 0,
                    classes[index].creatable != 0,
                });
            }

            for (std::size_t index = 0; index < node_count; ++index) {
                std::optional<std::size_t> parent;

                if (nodes[index].has_parent) {
                    if (nodes[index].parent >= node_count) {
                        throw std::runtime_error("native Roblox parent index is invalid");
                    }

                    parent = nodes[index].parent;
                }

                metadata.nodes.push_back({
                    native_text(nodes[index].name),
                    native_text(nodes[index].class_name),
                    parent,
                });
            }

            static_cast<instar::Engine *>(engine)->prepare_roblox(metadata);
            success = 1;
        });

        return success;
    }

    void instar_engine_check(void *engine, void *context, NativeReport report, NativeFailure failure) {
        if (!engine) {
            report_failure(context, failure, "native engine is unavailable");

            return;
        }

        run(context, failure, [&] {
            for (const auto &diagnostic : static_cast<instar::Engine *>(engine)->check()) {
                if (report) {
                    report(context, native_bytes(diagnostic.path), native_bytes(diagnostic.message),
                        native_range(diagnostic.range), diagnostic.error ? 1 : 0);
                }
            }
        });
    }

    void instar_engine_syntax(void *engine, NativeBytes path, void *context, NativeSyntax report, NativeFailure failure) {
        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            auto value = static_cast<instar::Engine *>(engine)->syntax(native_text(path));

            if (!value) {
                return;
            }

            std::vector<NativeBytes> parameters;

            for (const auto &parameter : value->parameters) {
                parameters.push_back(native_bytes(parameter));
            }

            report(context, native_bytes(value->description), parameters.data(), parameters.size());
        });
    }

    void instar_engine_type_at(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeType report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            auto value =
                static_cast<instar::Engine *>(engine)->type_at(native_text(path), {position.line, position.column});

            if (value) {
                report(context, native_bytes(value->description));
            }
        });
    }

    void instar_engine_hover(void *engine, NativeBytes path, NativePosition position, void *context, NativeType report,
        NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            auto value =
                static_cast<instar::Engine *>(engine)->hover(native_text(path), {position.line, position.column});

            if (value) {
                report(context, native_bytes(value->description));
            }
        });
    }

    void instar_engine_complete(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeCompletion report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &completion :
                static_cast<instar::Engine *>(engine)->complete(native_text(path), {position.line, position.column})) {
                report(context, native_bytes(completion.label), native_bytes(completion.description),
                    native_bytes(completion.documentation), native_bytes(completion.insert_text), completion.kind,
                    completion.deprecated ? 1 : 0);
            }
        });
    }

    void instar_engine_signature(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeSignature report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            auto value =
                static_cast<instar::Engine *>(engine)->signature(native_text(path), {position.line, position.column});

            if (!value) {
                return;
            }

            std::vector<NativeBytes> parameters;

            for (const auto &parameter : value->parameters) {
                parameters.push_back(native_bytes(parameter));
            }

            report(context, native_bytes(value->description), parameters.data(), parameters.size(),
                value->active_parameter);
        });
    }

    void instar_engine_definition(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeDestination report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            auto value =
                static_cast<instar::Engine *>(engine)->definition(native_text(path), {position.line, position.column});

            if (value) {
                report(context, native_bytes(value->path), native_range(value->range), native_bytes(value->name));
            }
        });
    }

    void instar_engine_type_definition(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeDestination report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            auto value = static_cast<instar::Engine *>(engine)->type_definition(
                native_text(path), {position.line, position.column});

            if (value) {
                report(context, native_bytes(value->path), native_range(value->range), native_bytes(value->name));
            }
        });
    }

    void instar_engine_implementations(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeDestination report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &value : static_cast<instar::Engine *>(engine)->implementations(
                     native_text(path), {position.line, position.column})) {
                report(context, native_bytes(value.path), native_range(value.range), native_bytes(value.name));
            }
        });
    }

    void instar_engine_references(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeDestination report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &value : static_cast<instar::Engine *>(engine)->references(
                     native_text(path), {position.line, position.column})) {
                report(context, native_bytes(value.path), native_range(value.range), native_bytes(value.name));
            }
        });
    }

    void instar_engine_annotations(
        void *engine, NativeBytes path, void *context, NativeAnnotation report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &value : static_cast<instar::Engine *>(engine)->annotations(native_text(path))) {
                report(context, native_bytes(value.path), {value.position.line, value.position.column},
                    native_bytes(value.text));
            }
        });
    }

    void instar_engine_calls(void *engine, NativeBytes path, void *context, NativeCall report, NativeFailure failure) {
        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &value : static_cast<instar::Engine *>(engine)->calls(native_text(path))) {
                report(context, native_bytes(value.target.path), native_range(value.target.range),
                    native_bytes(value.target.name), native_range(value.caller), value.container ? 1 : 0,
                    value.container ? native_range(*value.container) : NativeRange{{0, 0}, {0, 0}});
            }
        });
    }

    void instar_engine_extract(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeExtract report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &value :
                static_cast<instar::Engine *>(engine)->extract(native_text(path), {position.line, position.column})) {
                report(context, native_range(value.range), native_range(value.selection));
            }
        });
    }

    void instar_engine_tokens(
        void *engine, NativeBytes path, void *context, NativeSymbol report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &value : static_cast<instar::Engine *>(engine)->tokens(native_text(path))) {
                report(context, native_bytes(value.name), native_bytes(value.path), native_range(value.range),
                    native_range(value.selection), value.kind, value.declaration ? 1 : 0, value.modifiers);
            }
        });
    }

    void instar_engine_index(
        void *engine, NativeBytes path, void *context, NativeSymbol symbol, NativeCall call, NativeFailure failure) {

        if (!engine || (!symbol && !call)) {
            return;
        }

        run(context, failure, [&] {
            auto *value = static_cast<instar::Engine *>(engine);

            if (symbol) {
                for (const auto &entry : value->index(native_text(path))) {
                    symbol(context, native_bytes(entry.name), native_bytes(entry.path), native_range(entry.range),
                        native_range(entry.selection), entry.kind, entry.declaration ? 1 : 0, entry.modifiers);
                }
            }

            if (call) {
                for (const auto &entry : value->calls(native_text(path))) {
                    call(context, native_bytes(entry.target.path), native_range(entry.target.range),
                        native_bytes(entry.target.name), native_range(entry.caller), entry.container ? 1 : 0,
                        entry.container ? native_range(*entry.container) : NativeRange{{0, 0}, {0, 0}});
                }
            }
        });
    }

    void instar_engine_imports(void *engine, NativeBytes path, NativePosition position, void *context,
        NativeImport report, NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            for (const auto &value :
                static_cast<instar::Engine *>(engine)->imports(native_text(path), {position.line, position.column})) {
                report(context, native_bytes(value.name), native_bytes(value.label), native_bytes(value.target),
                    native_range(value.range));
            }
        });
    }

    void instar_engine_scope(void *engine, NativeBytes path, NativePosition position, void *context, NativeScope report,
        NativeFailure failure) {

        if (!engine || !report) {
            return;
        }

        run(context, failure, [&] {
            auto value =
                static_cast<instar::Engine *>(engine)->scope(native_text(path), {position.line, position.column});

            if (value) {
                report(context, value->name ? native_bytes(*value->name) : NativeBytes{nullptr, 0}, value->name ? 1 : 0,
                    value->kind.value_or(0), value->kind ? 1 : 0);
            }
        });
    }

    void instar_parse_aliases(
        NativeBytes source, int executable, void *context, NativeAlias alias, NativeFailure failure) {

        run(context, failure, [&] {
            auto result = instar::parse_aliases(native_text(source), executable != 0);

            if (result.error) {
                report_failure(context, failure, *result.error);
                return;
            }

            for (const auto &entry : result.aliases) {
                if (alias) {
                    alias(context, native_bytes(entry.name), native_bytes(entry.value));
                }
            }
        });
    }

    int instar_matches(NativeBytes pattern, NativeBytes source) {
        try {
            return std::regex_search(native_text(source), std::regex(native_text(pattern))) ? 1 : 0;
        } catch (...) {
            return -1;
        }
    }
}
