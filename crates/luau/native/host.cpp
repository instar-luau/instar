#include "host.hpp"

#include "Luau/Ast.h"
#include "Luau/RequireNavigator.h"
#include "frontend.hpp"
#include "instar-luau/src/bridge.rs.h"

#include <algorithm>
#include <stdexcept>
#include <utility>

namespace instar::luau {
    namespace {
        class Navigation final : public Luau::Require::NavigationContext {
        public:
            Navigation(const Context &context, Frontend &frontend, const Luau::ModuleInfo &requirer, std::string source)
                : context(context), frontend(frontend), requirer(requirer), source(std::move(source)) {}

            NavigateResult resetToRequirer() override {
                current = source;
                hierarchy = nullptr;

                return NavigateResult::Success;
            }

            NavigateResult jumpToAlias(const std::string &) override {
                return NavigateResult::NotFound;
            }

            NavigateResult toAliasOverride(const std::string &name) override {
                rust::String destination;
                const Status status = context.alias(source, name, destination);

                if (status == Status::Missing && name == "game") {
                    hierarchy = frontend.hierarchy(requirer.name);

                    return move(hierarchy ? hierarchy->global(InstanceQuery::Game, std::nullopt) : std::nullopt);
                }

                return move(status, destination);
            }

            NavigateResult toParent() override {
                if (hierarchy) {
                    return move(hierarchy->resolve(current, InstanceQuery::Parent, {}));
                }

                rust::String destination;
                const Status status = context.parent(current, destination);

                return move(status, destination);
            }

            NavigateResult toChild(const std::string &name) override {
                if (hierarchy) {
                    return move(hierarchy->resolve(current, InstanceQuery::Child, name));
                }

                rust::String destination;
                const Status status = context.child(current, name, destination);

                return move(status, destination);
            }

            std::string current;

        private:
            NavigateResult move(Status status, const rust::String &destination) {
                switch (status) {
                case Status::Found:
                    current = std::string(destination);

                    return NavigateResult::Success;

                case Status::Missing:
                    return NavigateResult::NotFound;

                case Status::Ambiguous:
                    return NavigateResult::Ambiguous;
                }

                throw std::invalid_argument("invalid Luau navigation status");
            }

            NavigateResult move(std::optional<std::string> destination) {
                if (!destination) {
                    return NavigateResult::NotFound;
                }

                current = std::move(*destination);

                return NavigateResult::Success;
            }

            const Context &context;
            Frontend &frontend;
            const Luau::ModuleInfo &requirer;
            const std::string source;
            const Instances *hierarchy = nullptr;
        };
    }

    Host::Host(const Context &context, const Flags &flags, Frontend &frontend)
        : callbacks(context), flags(flags), frontend(frontend) {}

    std::optional<Luau::SourceCode> Host::readSource(const Luau::ModuleName &name) {
        hierarchy = nullptr;

        auto type = Luau::SourceCode::Module;
        std::string source_name = name;

        if (const auto found = mappings.find(name); found != mappings.end()) {
            if (!found->second.source) {
                return std::nullopt;
            }

            source_name = *found->second.source;
            type = found->second.type;
        }

        Text source = callbacks.source(source_name);

        if (!source.present) {
            return std::nullopt;
        }

        std::string text(source.value);

        if (text.compare(0, 3, "\xef\xbb\xbf") == 0) {
            text.replace(0, 3, "   ");
        }

        return Luau::SourceCode{std::move(text), type};
    }

    std::string Host::getHumanReadableModuleName(const Luau::ModuleName &name) const {
        const auto found = mappings.find(name);

        return found != mappings.end() && found->second.source ? *found->second.source : name;
    }

    std::optional<Luau::ModuleInfo>
    Host::resolveModule(const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) {
        if (!context) {
            return std::nullopt;
        }

        if (const auto *literal = expression->as<Luau::AstExprConstantString>()) {
            Navigation navigation(callbacks, frontend, *context, getHumanReadableModuleName(context->name));
            Luau::Require::ErrorHandler errors;
            Luau::Require::Navigator navigator(navigation, errors);

            if (navigator.navigate(std::string(literal->value.data, literal->value.size)) !=
                Luau::Require::Navigator::Status::Success) {
                return std::nullopt;
            }

            const auto targets = modules(navigation.current);

            return Luau::ModuleInfo{targets.size() == 1 ? targets.front() : navigation.current};
        }

        InstanceQuery query;
        std::string name;

        if (const auto *global = expression->as<Luau::AstExprGlobal>()) {
            if (global->name == "game") {
                query = InstanceQuery::Game;
            } else if (global->name == "workspace") {
                query = InstanceQuery::Workspace;
            } else if (global->name == "script") {
                query = InstanceQuery::Script;
            } else {
                return std::nullopt;
            }

            hierarchy = frontend.hierarchy(context->name);
            const auto destination = hierarchy ? hierarchy->global(query, resolve(context->name).script) : std::nullopt;

            return destination ? std::optional<Luau::ModuleInfo>{Luau::ModuleInfo{*destination}} : std::nullopt;
        } else if (const auto *index = expression->as<Luau::AstExprIndexName>()) {
            name = index->index.value;
            query = name == "Parent" ? InstanceQuery::Parent : InstanceQuery::Property;
        } else if (const auto *index = expression->as<Luau::AstExprIndexExpr>()) {
            const auto *literal = index->index->as<Luau::AstExprConstantString>();

            if (!literal) {
                return std::nullopt;
            }

            name.assign(literal->value.data, literal->value.size);
            query = name == "Parent" ? InstanceQuery::Parent : InstanceQuery::Property;
        } else if (const auto *call = expression->as<Luau::AstExprCall>(); call && call->self && call->args.size > 0) {
            const auto *method = call->func->as<Luau::AstExprIndexName>();
            const auto *literal = call->args.data[0]->as<Luau::AstExprConstantString>();

            if (!method || !literal) {
                return std::nullopt;
            }

            name.assign(literal->value.data, literal->value.size);

            if (method->index == "GetService" && call->args.size == 1) {
                query = InstanceQuery::Service;
            } else if (method->index == "FindFirstAncestor" && call->args.size == 1) {
                query = InstanceQuery::Ancestor;
            } else if ((method->index == "WaitForChild" || method->index == "FindFirstChild") && call->args.size <= 2) {
                query = InstanceQuery::Child;

                if (method->index == "FindFirstChild" && call->args.size == 2) {
                    const auto *recursive = call->args.data[1]->as<Luau::AstExprConstantBool>();

                    if (recursive && recursive->value) {
                        query = InstanceQuery::Descendant;
                    }
                }
            } else {
                return std::nullopt;
            }
        } else {
            return std::nullopt;
        }

        const auto destination = hierarchy ? hierarchy->resolve(context->name, query, name) : std::nullopt;

        return destination ? std::optional<Luau::ModuleInfo>{Luau::ModuleInfo{*destination}} : std::nullopt;
    }

    const Luau::Config &Host::getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const {
        return resolve(name).configuration;
    }

    std::optional<std::string> Host::getEnvironmentForModule(const Luau::ModuleName &name) const {
        const auto environment = resolve(name).environment;

        if (environment) {
            frontend.environment(*environment);
        }

        return environment;
    }

    Environment Host::environment(const std::string &name) const {
        return callbacks.environment(name);
    }

    const Instances *
    Host::prepare(Luau::Frontend &native, const Luau::ModuleName &name, const Luau::ScopePtr &scope) const {
        const auto &settings = resolve(name);

        if (!settings.hierarchy) {
            return nullptr;
        }

        const auto key = std::make_pair(settings.environment.value_or(""), *settings.hierarchy);
        auto found = hierarchies.find(key);

        if (found == hierarchies.end()) {
            const auto environment = getEnvironmentForModule(name);
            const auto base = environment ? native.getEnvironmentScope(*environment) : native.globals.globalScope;
            const auto &instances = load(*settings.hierarchy);

            found = hierarchies
                        .emplace(
                            key,
                            std::make_unique<Instances>(
                                *settings.hierarchy,
                                base,
                                rust::Slice<const Instance>{instances.data(), instances.size()},
                                native.builtinTypes
                            )
                        )
                        .first;
        }

        if (scope) {
            found->second->bind(native.globals, scope, settings.script);
        }

        return found->second.get();
    }

    std::vector<std::string> Host::modules(const Luau::ModuleName &name) const {
        return resolve(name).modules;
    }

    void Host::invalidate(const Luau::ModuleName &name) {
        configurations.erase(name);
    }

    void Host::clear() {
        configurations.clear();
        hierarchies.clear();
        mappings.clear();
        models.clear();
        hierarchy = nullptr;
    }

    const Host::Settings &Host::resolve(const Luau::ModuleName &name) const {
        if (const auto found = configurations.find(name); found != configurations.end()) {
            return found->second;
        }

        if (const auto found = mappings.find(name); found != mappings.end()) {
            const auto mapping = found->second;

            if (!mapping.source) {
                static const Settings settings;

                return settings;
            }

            auto settings = resolve(*mapping.source);

            if (std::find(settings.modules.begin(), settings.modules.end(), name) != settings.modules.end()) {
                settings.hierarchy = mapping.hierarchy;
                settings.script = mapping.script;
            }

            settings.modules = {name};

            return configurations.emplace(name, std::move(settings)).first->second;
        }

        Configuration configuration = callbacks.configuration(name);

        try {
            flags.require({configuration.flags.data(), configuration.flags.size()});
        } catch (const std::invalid_argument &error) {
            throw std::invalid_argument(name + ": " + error.what());
        }

        Settings settings;
        auto &native = settings.configuration;
        native.parseOptions.allowDeclarationSyntax = true;

        switch (configuration.mode) {
        case Mode::NoCheck:
            native.mode = Luau::Mode::NoCheck;
            break;

        case Mode::NonStrict:
            native.mode = Luau::Mode::Nonstrict;
            break;

        case Mode::Strict:
            native.mode = Luau::Mode::Strict;
            break;

        default:
            throw std::invalid_argument("invalid Luau checking mode");
        }

        native.globals.reserve(configuration.globals.size());

        for (const rust::String &global : configuration.globals) {
            native.globals.emplace_back(std::string(global));
        }

        if (configuration.environment.present) {
            settings.environment = std::string(configuration.environment.value);
        }

        if (!configuration.locations.empty()) {
            const auto &first = configuration.locations[0];

            if (std::all_of(
                    configuration.locations.begin(),
                    configuration.locations.end(),
                    [&](const Location &location) { return location.hierarchy == first.hierarchy; }
                )) {
                settings.hierarchy = std::string(first.hierarchy);
            }

            if (configuration.locations.size() == 1 && first.has_script) {
                settings.script = first.script;
            }

            for (const Location &location : configuration.locations) {
                const auto &instances = load(std::string(location.hierarchy));

                if (!location.has_script) {
                    continue;
                }

                if (location.script >= instances.size()) {
                    throw std::invalid_argument("script index is outside its instance hierarchy");
                }

                auto identity = instance_identity(std::string(location.hierarchy), location.script);

                if (!mappings.at(identity).source) {
                    auto mapping = mappings.at(identity);
                    mapping.source = name;
                    identity += ":" + name;
                    mappings.insert_or_assign(identity, std::move(mapping));
                }

                if (std::find(settings.modules.begin(), settings.modules.end(), identity) == settings.modules.end()) {
                    settings.modules.push_back(std::move(identity));
                }
            }
        }

        if (settings.modules.empty()) {
            settings.modules.push_back(name);
        }

        return configurations.emplace(name, std::move(settings)).first->second;
    }

    const rust::Vec<Instance> &Host::load(const std::string &name) const {
        if (const auto found = models.find(name); found != models.end()) {
            return found->second;
        }

        auto instances = callbacks.hierarchy(name);

        for (size_t script = 0; script < instances.size(); ++script) {
            const auto &instance = instances[script];

            mappings.emplace(
                instance_identity(name, script),
                Mapping{instance.module.present && instance.class_name == "ModuleScript"
                            ? std::optional<std::string>{std::string(instance.module.value)}
                            : std::nullopt,
                    name,
                    script,
                    instance.class_name == "ModuleScript" ? Luau::SourceCode::Module : Luau::SourceCode::Script}
            );
        }

        return models.emplace(name, std::move(instances)).first->second;
    }
}
