#include "Luau/BuiltinDefinitions.h"
#include "Luau/Common.h"
#include "Luau/Error.h"
#include "Luau/Linter.h"
#include "Luau/Module.h"
#include "Luau/TypeAttach.h"
#include "operation.hpp"
#include "roblox.hpp"

#include <array>
#include <cstring>
#include <stdexcept>
#include <string_view>
#include <utility>
#include <vector>

namespace instar {
    namespace {
        std::array<uint32_t, 4> location(const Luau::Location &value) {
            return {value.begin.line, value.begin.column, value.end.line, value.end.column};
        }

        rust::Str borrowed(std::string_view value) {
            return rust::Str(value.data() ? value.data() : "", value.size());
        }

        template <typename T> void emit_fast_flags(Luau::FValue<T> *flags, bool boolean, const char *prefix, const char *dynamic_prefix, rust::Vec<FastFlag> &output) {
            for (Luau::FValue<T> *flag = flags; flag; flag = flag->next) {
                const std::string name = std::string(flag->dynamic ? dynamic_prefix : prefix) + flag->name;
                output.push_back(FastFlag{rust::String(name), boolean, boolean && bool(flag->value), boolean ? 0 : int32_t(flag->value)});
            }
        }

        template <typename T> bool flag_name_matches(const Luau::FValue<T> &flag, std::string_view name, std::string_view prefix, std::string_view dynamic_prefix) {
            const std::string_view selected_prefix = flag.dynamic ? dynamic_prefix : prefix;

            return name.size() == selected_prefix.size() + std::strlen(flag.name) && name.substr(0, selected_prefix.size()) == selected_prefix && name.substr(selected_prefix.size()) == flag.name;
        }
    } // namespace

    Checker::Checker(bool retain) : files(this), configurations(this), frontend(Luau::SolverMode::New, &files, &configurations, Luau::FrontendOptions{retain}) {
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
    }

    void Checker::diagnostic(
        std::string_view path, const Luau::Location &range, DiagnosticSeverity severity, std::string_view message, std::string_view rule, std::string_view related_path, const Luau::Location *related_location,
        std::string_view related_message
    ) {
        DiagnosticData value{};
        value.path = borrowed(path);
        value.location = location(range);
        value.severity = severity;
        value.message = borrowed(message);
        value.rule = borrowed(rule);
        value.has_related = related_location != nullptr;

        if (related_location) {
            value.related_path = borrowed(related_path);
            value.related_location = location(*related_location);
            value.related_message = borrowed(related_message);
        }

        if (!host || !host->diagnostic(value)) {
            throw CallbackFailure{};
        }
    }

    void Checker::emit(const Luau::TypeError &error, std::string_view checked) {
        const std::string_view owner = error.moduleName.empty() ? checked : std::string_view(error.moduleName);
        const std::string message = Luau::toString(error, Luau::TypeErrorToStringOptions{&files});
        const Luau::DuplicateTypeDefinition *duplicate = Luau::get<Luau::DuplicateTypeDefinition>(error);

        if (duplicate && duplicate->previousLocation) {
            diagnostic(owner, error.location, DiagnosticSeverity::DiagnosticError, message, {}, owner, &*duplicate->previousLocation, "previous definition");
        } else {
            diagnostic(owner, error.location, DiagnosticSeverity::DiagnosticError, message);
        }
    }

    void Checker::emit(std::string_view path, const Luau::LintWarning &warning, bool error) {
        const std::string message = std::string(Luau::LintWarning::getName(warning.code)) + ": " + warning.text;
        diagnostic(path, warning.location, error ? DiagnosticSeverity::DiagnosticError : DiagnosticSeverity::DiagnosticWarning, message, Luau::LintWarning::getName(warning.code));
    }

    std::optional<std::string> FileResolver::getEnvironmentForModule(const Luau::ModuleName &name) const {
        return checker->script_modules.count(name) ? std::optional<std::string>(name) : std::nullopt;
    }

    std::optional<Luau::SourceCode> FileResolver::readSource(const Luau::ModuleName &name) {
        origin = name;

        if (!checker->host) {
            throw CallbackFailure{};
        }

        const SourceData result = checker->host->source(borrowed(name));

        if (!result.success) {
            throw CallbackFailure{};
        }

        if (result.kind == SourceKind::SourceUnknown) {
            return std::nullopt;
        }

        if (result.kind != SourceKind::SourceScript && result.kind != SourceKind::SourceModule) {
            throw CallbackFailure{};
        }

        return Luau::SourceCode{std::string(reinterpret_cast<const char *>(result.bytes.data()), result.bytes.size()), static_cast<Luau::SourceCode::Type>(result.kind)};
    }

    std::optional<Luau::ModuleInfo> FileResolver::resolveModule(const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) {
        if (!expression || origin.empty()) {
            return std::nullopt;
        }

        if (!checker->host) {
            throw CallbackFailure{};
        }

        const Resolved result = checker->host->resolve(
            Resolution{
                borrowed(origin),
                context ? borrowed(context->name) : rust::Str{},
                context != nullptr,
                context && context->optional,
                location(expression->location),
            }
        );

        if (!result.success) {
            throw CallbackFailure{};
        }

        if (!result.present) {
            return std::nullopt;
        }

        return Luau::ModuleInfo{std::string(result.path), context && context->optional};
    }

    const Luau::Config &ConfigurationResolver::getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const {
        if (!checker->host) {
            throw CallbackFailure{};
        }

        const Configuration *configuration = checker->host->configuration(borrowed(name));

        if (!configuration) {
            throw CallbackFailure{};
        }

        return configuration->value;
    }

    std::unique_ptr<Configuration> configuration_create(rust::Slice<const uint8_t> source, Failure &failure) noexcept {
        std::unique_ptr<Configuration> result;

        failure = attempt([&] {
            Luau::Config configuration;
            Luau::ConfigOptions options;
            options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};

            if (const std::optional<std::string> message = Luau::parseConfig(std::string(reinterpret_cast<const char *>(source.data()), source.size()), configuration, options)) {
                throw std::invalid_argument(*message);
            }

            result = std::make_unique<Configuration>(Configuration{std::move(configuration)});
        });

        return result;
    }

    std::unique_ptr<Checker> checker_create(bool retain, Failure &failure) noexcept {
        std::unique_ptr<Checker> result;
        failure = attempt([&] { result = std::make_unique<Checker>(retain); });

        return result;
    }

    Failure fast_flags(rust::Vec<FastFlag> &output) noexcept {
        return attempt([&] {
            emit_fast_flags(Luau::FValue<bool>::list, true, "FFlag", "DFFlag", output);
            emit_fast_flags(Luau::FValue<int>::list, false, "FInt", "DFInt", output);
        });
    }

    Failure set_fast_flag(rust::Str name, bool boolean, bool bool_value, int32_t int_value) noexcept {
        return attempt([&] {
            const std::string_view value(name.data(), name.size());

            for (Luau::FValue<bool> *flag = Luau::FValue<bool>::list; flag; flag = flag->next) {
                if (flag_name_matches(*flag, value, "FFlag", "DFFlag")) {
                    if (!boolean) {
                        throw std::invalid_argument("fast flag type mismatch");
                    }

                    flag->value = bool_value;
                    return;
                }
            }

            for (Luau::FValue<int> *flag = Luau::FValue<int>::list; flag; flag = flag->next) {
                if (flag_name_matches(*flag, value, "FInt", "DFInt")) {
                    if (boolean) {
                        throw std::invalid_argument("fast flag type mismatch");
                    }

                    flag->value = int_value;
                    return;
                }
            }

            throw std::invalid_argument("unknown fast flag");
        });
    }

    Failure checker_mark_dirty(Checker &checker, rust::Str name) noexcept {
        return attempt([&] {
            const std::string value(name);

            if (checker.definition_names.count(value)) {
                throw std::invalid_argument("definition changes require a new checker");
            }

            checker.frontend.markDirty(value);
        });
    }

    Failure checker_clear_sources(Checker &checker) noexcept {
        return attempt([&] {
            std::vector<Luau::ModuleName> names;
            names.reserve(checker.frontend.sourceNodes.size());

            for (const auto &[name, _] : checker.frontend.sourceNodes) {
                if (!checker.definition_names.count(name)) {
                    names.push_back(name);
                }
            }

            checker.frontend.clearModules(names);
        });
    }

    Failure checker_register_roblox_classes(Checker &checker, rust::Slice<const RobloxClass> classes) noexcept {
        return attempt([&] {
            if (checker.globals_frozen || checker.roblox_classes_registered || !checker.frontend.sourceNodes.empty()) {
                throw std::invalid_argument("global changes require a new checker");
            }

            register_roblox_magic(checker.frontend.globals, classes);
            checker.roblox_classes_registered = true;
        });
    }

    Failure checker_register_roblox_tree(Checker &checker, rust::Slice<const RobloxNode> nodes) noexcept {
        return attempt([&] {
            if (nodes.empty()) {
                throw std::invalid_argument("invalid Roblox hierarchy request");
            }

            if (!checker.roblox_classes_registered) {
                throw std::invalid_argument("register Roblox classes before the hierarchy");
            }

            if (checker.globals_frozen || checker.roblox_tree_registered || !checker.frontend.sourceNodes.empty()) {
                throw std::invalid_argument("hierarchy changes require a new checker");
            }

            checker.script_modules = register_roblox_tree(checker.frontend, nodes);
            checker.roblox_tree_registered = true;
        });
    }

    Failure checker_freeze(Checker &checker) noexcept {
        return attempt([&] {
            Luau::freeze(checker.frontend.globals.globalTypes);
            checker.globals_frozen = true;
        });
    }

    Failure checker_load_definition(Checker &checker, Host &host, rust::Slice<const uint8_t> source, rust::Str package) noexcept {
        return attempt([&] {
            Operation operation(checker, host);
            auto &frontend = operation.frontend();
            const std::string name(package);

            if (checker.globals_frozen || checker.roblox_classes_registered || !frontend.sourceNodes.empty() || frontend.sourceModules.count(name)) {
                throw std::invalid_argument("definition changes require a new checker");
            }

            Luau::LoadDefinitionFileResult result =
                frontend.loadDefinitionFile(frontend.globals, frontend.globals.globalScope, std::string_view(reinterpret_cast<const char *>(source.data()), source.size()), name, false);

            for (const Luau::ParseError &parse_error : result.parseResult.errors) {
                checker.diagnostic(name, parse_error.getLocation(), DiagnosticSeverity::DiagnosticError, parse_error.getMessage());
            }

            if (result.module) {
                for (const Luau::TypeError &type_error : result.module->errors) {
                    checker.emit(type_error, name);
                }
            }

            if (!result.success) {
                throw DefinitionFailure{};
            }

            frontend.sourceModules[name] = std::make_shared<Luau::SourceModule>(std::move(result.sourceModule));
            frontend.moduleResolver.setModule(name, std::move(result.module));
            checker.definition_names.insert(name);
        });
    }

    Failure checker_parse(Checker &checker, Host &host, rust::Str name) noexcept {
        return attempt([&] {
            Operation operation(checker, host);
            const std::string value(name);

            if (!checker.definition_names.count(value)) {
                operation.frontend().parse(value);
            }
        });
    }

    Failure checker_parse_diagnostics(Checker &checker, Host &host, rust::Str name) noexcept {
        return attempt([&] {
            Operation operation(checker, host);
            const std::string value(name);
            const Luau::SourceModule *source = operation.frontend().getSourceModule(value);

            if (!source) {
                throw std::invalid_argument("source unavailable after parse");
            }

            for (const Luau::ParseError &parse_error : source->parseErrors) {
                checker.diagnostic(value, parse_error.getLocation(), DiagnosticSeverity::DiagnosticError, parse_error.getMessage());
            }
        });
    }

    Failure checker_prepare(Checker &checker, Host &host, rust::Str name, rust::Vec<rust::String> &timeouts) noexcept {
        return attempt([&] {
            Operation operation(checker, host);
            auto &frontend = operation.frontend();
            const std::string path(name);

            if (checker.definition_names.count(path)) {
                return;
            }

            frontend.queueModuleCheck(path);
            frontend.checkQueuedModules();
            std::vector<Luau::ModuleName> pending{path};
            std::unordered_set<Luau::ModuleName> seen;

            while (!pending.empty()) {
                const Luau::ModuleName current = std::move(pending.back());
                pending.pop_back();

                if (!seen.insert(current).second) {
                    continue;
                }

                const auto module = frontend.moduleResolver.getModule(current);

                if (module && module->timeout) {
                    timeouts.push_back(rust::String(current));
                }

                const auto node = frontend.sourceNodes.find(current);

                if (node != frontend.sourceNodes.end()) {
                    for (const Luau::ModuleName &dependency : node->second->requireSet) {
                        pending.push_back(dependency);
                    }
                }
            }
        });
    }

    Failure checker_check(Checker &checker, Host &host, rust::Str name) noexcept {
        return attempt([&] {
            Operation operation(checker, host);
            auto &frontend = operation.frontend();
            const std::string path(name);

            if (checker.definition_names.count(path)) {
                return;
            }

            const auto module = frontend.moduleResolver.getModule(path);

            if (frontend.isDirty(path) || !module || module->cancelled) {
                throw std::invalid_argument("semantic preparation required before checking");
            }

            for (const Luau::TypeError &type_error : module->errors) {
                checker.emit(type_error, path);
            }
        });
    }

    Failure checker_lint(Checker &checker, Host &host, rust::Str name) noexcept {
        return attempt([&] {
            Operation operation(checker, host);
            auto &frontend = operation.frontend();
            const std::string path(name);

            if (checker.definition_names.count(path)) {
                return;
            }

            const Luau::SourceModule *source = frontend.getSourceModule(path);
            const auto module = frontend.moduleResolver.getModule(path);

            if (frontend.isDirty(path) || !source || !module || module->cancelled) {
                throw std::invalid_argument("semantic preparation required before linting");
            }

            if (!frontend.options.retainFullTypeGraphs) {
                throw std::invalid_argument("linting requires retained full type graphs");
            }

            if (!source->parseErrors.empty()) {
                return;
            }

            if (!source->root) {
                throw std::invalid_argument("prepared syntax tree unavailable for linting");
            }

            const Luau::Config config = checker.configurations.getConfig(path, {});
            const Luau::Mode mode = source->mode.value_or(config.mode);
            Luau::LintOptions options = config.enabledLint;
            options.warningMask &= ~Luau::LintWarning::parseMask(source->hotcomments);

            if (mode != Luau::Mode::NoCheck) {
                options.disableWarning(Luau::LintWarning::Code_UnknownGlobal);
            }

            if (mode == Luau::Mode::Strict) {
                options.disableWarning(Luau::LintWarning::Code_ImplicitReturn);
            }

            Luau::ScopePtr environment = source->environmentName ? frontend.getEnvironmentScope(*source->environmentName) : frontend.globals.globalScope;

            if (!config.globals.empty()) {
                environment = std::make_shared<Luau::Scope>(environment);

                for (const std::string &global : config.globals) {
                    const Luau::AstName name = source->names->get(global.c_str());

                    if (name.value) {
                        environment->bindings[name].typeId = frontend.builtinTypes->anyType;
                    }
                }
            }

            const auto warnings = Luau::lint(source->root, *source->names, environment, module.get(), source->hotcomments, options);

            for (const Luau::LintWarning &warning : warnings) {
                checker.emit(path, warning, config.lintErrors || config.fatalLint.isEnabled(warning.code));
            }
        });
    }

    Failure checker_globals(const Checker &checker, Items &output) noexcept {
        return attempt([&] {
            for (const auto &[name, _] : checker.frontend.globals.globalScope->bindings) {
                if (!output.item(borrowed(name.c_str()))) {
                    throw CallbackFailure{};
                }
            }
        });
    }

    Failure checker_modules(const Checker &checker, Items &output) noexcept {
        return attempt([&] {
            for (const auto &[name, _] : checker.frontend.sourceNodes) {
                if (!output.item(borrowed(name))) {
                    throw CallbackFailure{};
                }
            }
        });
    }

    Failure checker_attach_type_data(Checker &checker, rust::Str name) noexcept {
        return attempt([&] {
            const std::string value(name);
            Luau::SourceModule *source = checker.frontend.getSourceModule(value);
            Luau::ModulePtr module = checker.frontend.moduleResolver.getModule(value);

            if (!source || !module) {
                throw std::invalid_argument("checked module unavailable for type data");
            }

            Luau::attachTypeData(*source, *module);
        });
    }
} // namespace instar
