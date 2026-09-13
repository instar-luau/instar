#include "roblox.hpp"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/ConstraintSolver.h"
#include "Luau/Module.h"
#include "Luau/TypeInfer.h"
#include "Luau/TypePack.h"
#include <algorithm>
#include <stdexcept>

namespace {
struct Instances : Luau::ClassUserData {
    std::map<std::string, Luau::TypeId> children;
};

std::optional<Luau::TypeId> instance(const Luau::Scope &scope, const std::string &name) {
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

    return parent && type && Luau::isSubclass(type, parent) ? std::optional<Luau::TypeId>{target->type} : std::nullopt;
}

void report(const Luau::MagicFunctionCallContext &context, const Luau::Location &location, const std::string &message) {
    if (context.constraint->moduleName) {
        context.solver->reportError(Luau::GenericError{message}, location, *context.constraint->moduleName);
    } else {
        context.solver->DEPRECATED_reportError(Luau::TypeError{location, Luau::GenericError{message}});
    }
}

struct Predicate : Luau::MagicFunction {
    std::optional<Luau::WithPredicate<Luau::TypePackId>>
    handleOldSolver(Luau::TypeChecker &checker, const Luau::ScopePtr &scope, const Luau::AstExprCall &call,
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
        auto type = instance(*scope, name);

        if (!type) {
            checker.reportError(
                Luau::TypeError{literal->location, Luau::GenericError{"Unknown instance class '" + name + "'"}});
        }

        if (!value || !type) {
            return std::nullopt;
        }

        return Luau::WithPredicate<Luau::TypePackId>{
            checker.currentModule->internalTypes->addTypePack({checker.booleanType}),
            {Luau::IsAPredicate{std::move(*value), call.location, *type}}};
    }

    bool infer(const Luau::MagicFunctionCallContext &context) override {
        if (context.callSite->args.size == 1) {
            if (auto literal = context.callSite->args.data[0]->as<Luau::AstExprConstantString>()) {
                std::string name(literal->value.data, literal->value.size);

                if (!instance(*context.solver->rootScope, name)) {
                    report(context, literal->location, "Unknown instance class '" + name + "'");
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

        auto type = instance(*context.scope, std::string(literal->value.data, literal->value.size));

        if (type) {
            Luau::asMutable(*context.discriminantTypes[0])->ty.emplace<Luau::BoundType>(*type);
        }
    }
};

enum class LookupKind { Child, Class, Service, Constructor };

struct Lookup : Luau::MagicFunction {
    LookupKind kind;
    explicit Lookup(LookupKind kind) : kind(kind) {}

    std::optional<Luau::TypeId> lookup(const Luau::Scope &scope, const Luau::AstExprCall &call,
                                       std::optional<Luau::TypeId> receiver) {
        if (call.args.size < 1) {
            return std::nullopt;
        }

        auto literal = call.args.data[0]->as<Luau::AstExprConstantString>();

        if (!literal) {
            return std::nullopt;
        }

        std::string name(literal->value.data, literal->value.size);
        auto result = instance(scope, name);

        if (kind != LookupKind::Child) {
            if (!result) {
                return std::nullopt;
            }

            auto type = Luau::get<Luau::ExternType>(Luau::follow(*result));

            auto tagged = [&](const char *tag) {
                return std::find(type->tags.begin(), type->tags.end(), tag) != type->tags.end();
            };

            if ((kind == LookupKind::Service && tagged("RobloxClass") && !tagged("RobloxService")) ||
                (kind == LookupKind::Constructor && tagged("RobloxNotCreatable"))) {
                return std::nullopt;
            }
        }

        if (receiver && (kind == LookupKind::Child || kind == LookupKind::Service)) {
            if (auto type = Luau::get<Luau::ExternType>(Luau::follow(*receiver))) {
                if (auto mapped = std::dynamic_pointer_cast<Instances>(type->userData)) {
                    if (kind == LookupKind::Child) {
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

        if (kind == LookupKind::Child) {
            return std::nullopt;
        }

        return result;
    }

    std::optional<Luau::WithPredicate<Luau::TypePackId>>
    handleOldSolver(Luau::TypeChecker &checker, const Luau::ScopePtr &scope, const Luau::AstExprCall &call,
                    Luau::WithPredicate<Luau::TypePackId> arguments) override {
        auto result = lookup(*scope, call, Luau::first(arguments.type));

        if (!result) {
            if (kind != LookupKind::Child && call.args.size && call.args.data[0]->is<Luau::AstExprConstantString>()) {
                checker.reportError(Luau::TypeError{call.args.data[0]->location,
                                                    Luau::GenericError{"Invalid Roblox class for this operation"}});
            }

            return std::nullopt;
        }

        if (kind == LookupKind::Class) {
            result = checker.currentModule->internalTypes->addType(Luau::UnionType{{*result, checker.nilType}});
        }

        return Luau::WithPredicate<Luau::TypePackId>{checker.currentModule->internalTypes->addTypePack({*result})};
    }

    bool infer(const Luau::MagicFunctionCallContext &context) override {
        auto result = lookup(*context.solver->rootScope, *context.callSite, Luau::first(context.arguments));

        if (!result) {
            if (kind != LookupKind::Child && context.callSite->args.size &&
                context.callSite->args.data[0]->is<Luau::AstExprConstantString>()) {
                report(context, context.callSite->args.data[0]->location, "Invalid Roblox class for this operation");
            }

            return false;
        }

        if (kind == LookupKind::Class) {
            result = context.solver->arena->addType(Luau::UnionType{{*result, context.solver->builtinTypes->nilType}});
        }

        Luau::asMutable(context.result)->ty.emplace<Luau::BoundTypePack>(context.solver->arena->addTypePack({*result}));

        return true;
    }
};

void attach(Luau::TypeId type, const std::shared_ptr<Luau::MagicFunction> &magic) {
    type = Luau::follow(type);

    if (Luau::get<Luau::FunctionType>(type)) {
        Luau::attachMagicFunction(type, magic);
    } else if (auto intersection = Luau::get<Luau::IntersectionType>(type)) {
        for (auto part : intersection->parts) {
            attach(part, magic);
        }
    }
}

void attach(Luau::ExternType &type, const std::string &name, std::shared_ptr<Luau::MagicFunction> magic) {
    auto property = Luau::lookupExternTypeProp(&type, name);

    if (property && property->readTy) {
        attach(*property->readTy, magic);
    }
}

Luau::TypeId external(Luau::Frontend &frontend, const std::string &name,
                      std::optional<Luau::TypeId> parent = std::nullopt) {
    return frontend.globals.globalTypes.addType(Luau::ExternType{name, {}, parent, {}, {}, {}, "roblox", {}});
}
} // namespace

std::vector<Luau::TypeId> prepareRoblox(Luau::Frontend &frontend, const RobloxMetadata &metadata) {
    auto &arena = frontend.globals.globalTypes;
    auto scope = frontend.globals.globalScope;

    for (auto name : {"Instance", "Enum", "EnumItem"}) {
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

    for (size_t index = 0;; ++index) {
        auto name = metadata("enumeration", index);

        if (!name) {
            break;
        }

        if (auto binding = scope->lookupType("Enum" + *name)) {
            scope->importedTypeBindings["Enum"][*name] = *binding;
        }
    }

    for (size_t index = 0;; ++index) {
        auto record = metadata("class", index);

        if (!record) {
            break;
        }

        auto separator = record->find('\0');
        auto binding = scope->lookupType(record->substr(0, separator));

        if (!binding) {
            continue;
        }

        if (auto type = Luau::getMutable<Luau::ExternType>(Luau::follow(binding->type))) {
            type->tags.push_back("RobloxClass");

            if (record->at(separator + 1) == '1') {
                type->tags.push_back("RobloxService");
            }

            if (record->at(separator + 2) == '0') {
                type->tags.push_back("RobloxNotCreatable");
            }
        }
    }

    auto instance = scope->lookupType("Instance");

    if (instance) {
        auto type = Luau::getMutable<Luau::ExternType>(Luau::follow(instance->type));

        if (type) {
            attach(*type, "IsA", std::make_shared<Predicate>());

            attach(*type, "WaitForChild", std::make_shared<Lookup>(LookupKind::Child));

            attach(*type, "FindFirstChild", std::make_shared<Lookup>(LookupKind::Child));

            for (auto name : {"FindFirstChildOfClass", "FindFirstChildWhichIsA", "FindFirstAncestorOfClass",
                              "FindFirstAncestorWhichIsA"}) {
                attach(*type, name, std::make_shared<Lookup>(LookupKind::Class));
            }

            for (auto &binding : scope->exportedTypeBindings) {
                if (auto derived = Luau::getMutable<Luau::ExternType>(Luau::follow(binding.second.type))) {
                    auto base = derived;

                    while (base != type && base && base->parent) {
                        base = Luau::getMutable<Luau::ExternType>(Luau::follow(*base->parent));
                    }

                    if (base != type) {
                        continue;
                    }

                    bool ancestor = false;

                    for (const auto &candidate : scope->exportedTypeBindings) {
                        if (auto child = Luau::get<Luau::ExternType>(Luau::follow(candidate.second.type));
                            child && child->parent &&
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
    }

    if (auto binding = scope->lookup(Luau::Symbol{Luau::AstName("Instance")})) {
        if (auto table = Luau::get<Luau::TableType>(Luau::follow(*binding))) {
            auto constructor = table->props.find("new");

            if (constructor != table->props.end() && constructor->second.readTy) {
                attach(*constructor->second.readTy, std::make_shared<Lookup>(LookupKind::Constructor));
            }
        }
    }

    if (auto model = scope->lookupType("DataModel")) {
        if (auto type = Luau::getMutable<Luau::ExternType>(Luau::follow(model->type))) {
            attach(*type, "GetService", std::make_shared<Lookup>(LookupKind::Service));
        }
    }

    for (const auto &entry : std::vector<std::pair<std::string, std::vector<const char *>>>{
             {"DataModel", {"game", "Game"}}, {"Workspace", {"workspace", "Workspace"}}}) {
        if (auto type = scope->lookupType(entry.first)) {
            for (auto name : entry.second) {
                scope->bindings[Luau::AstName(name)] = Luau::Binding{type->type};
            }
        }
    }

    std::vector<Luau::TypeId> types;
    std::vector<std::string> names;
    std::vector<std::optional<size_t>> parents;

    for (size_t index = 0;; ++index) {
        auto record = metadata("node", index);

        if (!record) {
            break;
        }

        auto separator = record->find('\0');
        auto end = record->find('\0', separator + 1);
        auto name = record->substr(0, separator);
        auto classname = record->substr(separator + 1, end - separator - 1);
        auto base = scope->lookupType(classname);

        if (!base || !Luau::get<Luau::ExternType>(Luau::follow(base->type))) {
            throw std::runtime_error("missing Roblox instance type " + classname);
        }

        types.push_back(external(frontend, classname, base->type));

        Luau::getMutable<Luau::ExternType>(types.back())->userData = std::make_shared<Instances>();

        names.push_back(name);
        auto parent = record->substr(end + 1);

        parents.push_back(parent.empty() ? std::nullopt : std::optional<size_t>{std::stoull(parent)});
    }

    for (size_t index = 0; index < types.size(); ++index) {
        auto type = Luau::getMutable<Luau::ExternType>(types[index]);
        auto inherited = Luau::lookupExternTypeProp(type, "Parent");

        if (inherited) {
            auto replacement = *inherited;

            if (replacement.readTy) {
                replacement.readTy = parents[index] ? types.at(*parents[index]) : frontend.builtinTypes->nilType;
            }

            type->props["Parent"] = std::move(replacement);
        }

        if (parents[index]) {
            auto parent = types.at(*parents[index]);
            auto type = Luau::getMutable<Luau::ExternType>(parent);

            std::static_pointer_cast<Instances>(type->userData)->children[names[index]] = types[index];

            if (!Luau::lookupExternTypeProp(type, names[index])) {
                type->props[names[index]] = Luau::Property::readonly(types[index]);
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

    return types;
}
