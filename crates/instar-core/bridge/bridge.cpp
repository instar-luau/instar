#include "Luau/AstJsonEncoder.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Config.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "Luau/Linter.h"
#include "Luau/LuauConfig.h"
#include "Luau/Module.h"
#include "Luau/PrettyPrinter.h"
#include "Luau/Scope.h"
#include "Luau/ToString.h"
#include "Luau/TypeAttach.h"
#include "lua.h"
#include "lualib.h"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <new>
#include <optional>
#include <string>
#include <string_view>
#include <utility>

extern "C" {
    struct InstarSlice {
        const char *data;
        size_t length;
    };

    struct InstarString {
        char *data;
        size_t length;
    };

    struct InstarFrontendOptions {
        uint8_t old_solver;
        uint8_t retain_full_type_graphs;
        uint8_t for_autocomplete;
        uint8_t run_lint_checks;
    };

    struct InstarConfigurationOptions {
        uint8_t compat;
        uint8_t has_alias_options;
        uint8_t overwrite_aliases;
        uint8_t has_config_location;
        InstarSlice config_location;
    };

    struct InstarDefinitionOptions {
        uint8_t capture_comments;
        uint8_t type_check_for_autocomplete;
    };

    struct InstarTypeCheckLimits {
        uint8_t has_finish_time;
        double finish_time;
        uint8_t has_instantiation_child_limit;
        int32_t instantiation_child_limit;
        uint8_t has_unifier_iteration_limit;
        int32_t unifier_iteration_limit;
        const void *cancellation_token;
    };

    using InstarSourceCallback = uint8_t (*)(void *, InstarSlice, InstarSlice *, uint32_t *);
    using InstarConfigurationCallback = uint8_t (*)(void *, InstarSlice, const void **);

    using InstarResolveCallback = uint8_t (*)(
        void *, InstarSlice, uint8_t, InstarSlice, InstarTypeCheckLimits, InstarSlice *, uint8_t *, uint8_t *);

    using InstarDiagnosticCallback = uint8_t (*)(
        void *, InstarSlice, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, int32_t, uint32_t, uint8_t, InstarSlice);

    using InstarAliasCallback = uint8_t (*)(void *, InstarSlice, InstarSlice, InstarSlice);
    using InstarItemCallback = uint8_t (*)(void *, InstarSlice);

    struct InstarCallbacks {
        InstarSourceCallback source;
        InstarConfigurationCallback configuration;
        InstarResolveCallback resolve;
        InstarDiagnosticCallback diagnostic;
    };
}

namespace {
    constexpr uint32_t native_type_error = 1;
    constexpr uint32_t native_parse_error = 2;
    constexpr uint32_t native_lint_warning = 3;

    struct CallbackFailure final : std::exception {
        const char *what() const noexcept override { return "native callback failed"; }
    };

    std::optional<std::string_view> view(InstarSlice input) {
        if (!input.data && input.length != 0) {
            return std::nullopt;
        }

        return std::string_view(input.data ? input.data : "", input.length);
    }

    InstarSlice slice(std::string_view input) {
        return {input.data(), input.size()};
    }

    InstarTypeCheckLimits limits(const Luau::TypeCheckLimits &input) {
        return {
            uint8_t(input.finishTime.has_value()),
            input.finishTime.value_or(0.0),
            uint8_t(input.instantiationChildLimit.has_value()),
            input.instantiationChildLimit.value_or(0),
            uint8_t(input.unifierIterationLimit.has_value()),
            input.unifierIterationLimit.value_or(0),
            input.cancellationToken.get(),
        };
    }

    void output(InstarString *destination, std::string_view value) {
        destination->data = nullptr;
        destination->length = 0;

        if (value.empty()) {
            return;
        }

        char *data = static_cast<char *>(std::malloc(value.size()));

        if (!data) {
            throw std::bad_alloc();
        }

        std::memcpy(data, value.data(), value.size());
        destination->data = data;
        destination->length = value.size();
    }

    int failure(InstarString *error, std::string_view message) {
        if (error) {
            output(error, message);
        }

        return 1;
    }

    int failure_noexcept(InstarString *error, const char *message) noexcept {
        if (!error) {
            return 1;
        }

        error->data = nullptr;
        error->length = 0;

        const size_t length = std::strlen(message);

        if (length == 0) {
            return 1;
        }

        char *data = static_cast<char *>(std::malloc(length));

        if (!data) {
            return 1;
        }

        std::memcpy(data, message, length);
        error->data = data;
        error->length = length;

        return 1;
    }

    int exception_noexcept(InstarString *error) noexcept {
        try {
            throw;
        } catch (const std::exception &value) {
            return failure_noexcept(error, value.what());
        } catch (...) {
            return failure_noexcept(error, "native exception");
        }
    }

    struct Configuration {
        Luau::Config value;
    };

    std::optional<Luau::ConfigOptions::AliasOptions> alias_options(const InstarConfigurationOptions &input) {
        if (!input.has_alias_options) {
            return std::nullopt;
        }

        Luau::ConfigOptions::AliasOptions result;
        result.overwriteAliases = input.overwrite_aliases != 0;

        if (input.has_config_location) {
            const std::optional<std::string_view> location = view(input.config_location);

            if (!location) {
                throw CallbackFailure{};
            }

            result.configLocation = std::string(*location);
        }

        return result;
    }

    struct Checker;

    struct FileResolver final : Luau::FileResolver {
        explicit FileResolver(Checker *checker) : checker(checker) {}

        std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override;

        std::optional<Luau::ModuleInfo> resolveModule(
            const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override;

        std::string getHumanReadableModuleName(const Luau::ModuleName &name) const override;

        Checker *checker;
    };

    struct ConfigurationResolver final : Luau::ConfigResolver {
        explicit ConfigurationResolver(Checker *checker) : checker(checker) {}

        const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override;

        Checker *checker;
    };

    struct Checker {
        Checker(const InstarCallbacks &callbacks, void *context, const InstarFrontendOptions &options)
            : callbacks(callbacks), context(context), files(this), configurations(this),
              frontend(options.old_solver != 0 ? Luau::SolverMode::Old : Luau::SolverMode::New, &files, &configurations,
                  Luau::FrontendOptions{
                      options.retain_full_type_graphs != 0,
                      options.for_autocomplete != 0,
                      options.run_lint_checks != 0,
                  }) {}

        bool emit_diagnostic(std::string_view path, const Luau::Location &location, uint32_t native_kind, int32_t code,
            uint32_t variant, uint8_t is_error, std::string_view message) {

            if (!callbacks.diagnostic(context, slice(path), location.begin.line, location.begin.column,
                    location.end.line, location.end.column, native_kind, code, variant, is_error, slice(message))) {
                return false;
            }

            return true;
        }

        bool emit(const Luau::TypeError &error) {
            const std::string message = Luau::toString(error, Luau::TypeErrorToStringOptions{&files});

            return emit_diagnostic(error.moduleName, error.location, native_type_error, error.code(),
                static_cast<uint32_t>(error.data.index()), 1, message);
        }

        bool emit(std::string_view path, const Luau::LintWarning &warning, bool is_error) {
            return emit_diagnostic(path, warning.location, native_lint_warning, static_cast<int32_t>(warning.code), 0,
                uint8_t(is_error), warning.text);
        }

        bool emit(const Luau::CheckResult &result, std::string_view path) {
            for (const Luau::TypeError &error : result.errors) {
                if (!emit(error)) {
                    return false;
                }
            }

            for (const Luau::LintWarning &warning : result.lintResult.errors) {
                if (!emit(path, warning, true)) {
                    return false;
                }
            }

            for (const Luau::LintWarning &warning : result.lintResult.warnings) {
                if (!emit(path, warning, false)) {
                    return false;
                }
            }

            return true;
        }

        InstarCallbacks callbacks;
        void *context;
        FileResolver files;
        ConfigurationResolver configurations;
        Luau::Frontend frontend;
        std::optional<Luau::CheckResult> check_result;
    };

    std::optional<Luau::SourceCode> FileResolver::readSource(const Luau::ModuleName &name) {
        InstarSlice source{};
        uint32_t type = uint32_t(Luau::SourceCode::None);

        if (!checker->callbacks.source(checker->context, slice(name), &source, &type)) {
            throw CallbackFailure{};
        }

        const std::optional<std::string_view> source_view = view(source);

        if (!source_view) {
            throw CallbackFailure{};
        }

        if (type == uint32_t(Luau::SourceCode::None)) {
            return std::nullopt;
        }

        if (type > uint32_t(Luau::SourceCode::Script)) {
            throw CallbackFailure{};
        }

        return Luau::SourceCode{std::string(*source_view), static_cast<Luau::SourceCode::Type>(type)};
    }

    std::optional<Luau::ModuleInfo> FileResolver::resolveModule(
        const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &type_check_limits) {

        if (!context || !expression) {
            return std::nullopt;
        }

        const std::string encoded = Luau::toJson(expression);
        InstarSlice resolved{};
        uint8_t resolved_optional = 0;
        uint8_t present = 0;
        const InstarTypeCheckLimits native_limits = limits(type_check_limits);

        if (!checker->callbacks.resolve(checker->context, slice(context->name), uint8_t(context->optional),
                slice(encoded), native_limits, &resolved, &resolved_optional, &present)) {
            throw CallbackFailure{};
        }

        if (!present) {
            return std::nullopt;
        }

        const std::optional<std::string_view> resolved_view = view(resolved);

        if (!resolved_view) {
            throw CallbackFailure{};
        }

        return Luau::ModuleInfo{std::string(*resolved_view), resolved_optional != 0};
    }

    std::string FileResolver::getHumanReadableModuleName(const Luau::ModuleName &name) const {
        return name;
    }

    const Luau::Config &ConfigurationResolver::getConfig(
        const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const {
        const void *configuration = nullptr;

        if (!checker->callbacks.configuration(checker->context, slice(name), &configuration) || !configuration) {
            throw CallbackFailure{};
        }

        return static_cast<const Configuration *>(configuration)->value;
    }

    void destroy_state(lua_State *state) noexcept {
        if (state) {
            lua_close(state);
        }
    }
} // namespace

extern "C" {
    void instar_string_destroy(InstarString value) {
        std::free(value.data);
    }

    Configuration *instar_configuration_new(InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            return new Configuration;
        } catch (...) {
            exception_noexcept(error);

            return nullptr;
        }
    }

    void instar_configuration_destroy(Configuration *configuration) {
        delete configuration;
    }

    int instar_configuration_parse_json(Configuration *configuration, InstarSlice source,
        const InstarConfigurationOptions *options, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (!configuration || !options) {
                return failure(error, "invalid configuration request");
            }

            const std::optional<std::string_view> source_view = view(source);

            if (!source_view) {
                return failure(error, "invalid configuration source");
            }

            Luau::ConfigOptions parse_options;
            parse_options.compat = options->compat != 0;
            parse_options.aliasOptions = alias_options(*options);

            if (const std::optional<std::string> parse_error =
                    Luau::parseConfig(std::string(*source_view), configuration->value, parse_options)) {
                return failure(error, *parse_error);
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_configuration_set_capture_comments(
        Configuration *configuration, uint8_t capture_comments, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (!configuration) {
                return failure(error, "invalid configuration");
            }

            configuration->value.parseOptions.captureComments = capture_comments != 0;

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_configuration_set_mode(Configuration *configuration, uint32_t mode, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!configuration || mode > uint32_t(Luau::Mode::Definition)) {
                return failure(error, "invalid configuration mode");
            }

            configuration->value.mode = static_cast<Luau::Mode>(mode);

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_configuration_extract_luau(Configuration *configuration, InstarSlice source,
        const InstarConfigurationOptions *options, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (!configuration || !options) {
                return failure(error, "invalid configuration request");
            }

            const std::optional<std::string_view> source_view = view(source);

            if (!source_view) {
                return failure(error, "invalid configuration source");
            }

            if (const std::optional<std::string> parse_error = Luau::extractLuauConfig(
                    std::string(*source_view), configuration->value, alias_options(*options), {})) {
                return failure(error, *parse_error);
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_configuration_aliases(
        const Configuration *configuration, InstarAliasCallback callback, void *context, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (!configuration || !callback) {
                return failure(error, "invalid alias request");
            }

            for (const auto &[name, alias] : configuration->value.aliases) {
                if (!callback(context, slice(name), slice(alias.originalCase), slice(alias.value))) {
                    return 2;
                }
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    Checker *instar_checker_new(
        const InstarCallbacks *callbacks, void *context, const InstarFrontendOptions *options, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (!callbacks || !options || !callbacks->source || !callbacks->configuration || !callbacks->resolve ||
                !callbacks->diagnostic) {
                failure(error, "invalid checker callbacks");

                return nullptr;
            }

            return new Checker(*callbacks, context, *options);
        } catch (...) {
            exception_noexcept(error);

            return nullptr;
        }
    }

    void instar_checker_destroy(Checker *checker) {
        delete checker;
    }

    int instar_checker_register_builtins(Checker *checker, uint8_t for_autocomplete, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!checker) {
                return failure(error, "null checker");
            }

            Luau::registerBuiltinGlobals(checker->frontend, checker->frontend.globals, for_autocomplete != 0);

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_freeze(Checker *checker, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!checker) {
                return failure(error, "null checker");
            }

            Luau::freeze(checker->frontend.globals.globalTypes);

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_load_definition(Checker *checker, InstarSlice source, InstarSlice package_name,
        const InstarDefinitionOptions *options, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> source_view = view(source);
            const std::optional<std::string_view> package_view = view(package_name);

            if (!checker || !source_view || !package_view || !options) {
                return failure(error, "invalid definition request");
            }

            Luau::LoadDefinitionFileResult result = checker->frontend.loadDefinitionFile(checker->frontend.globals,
                checker->frontend.globals.globalScope, *source_view, std::string(*package_view),
                options->capture_comments != 0, options->type_check_for_autocomplete != 0);

            for (const Luau::ParseError &parse_error : result.parseResult.errors) {
                if (!checker->emit_diagnostic(*package_view, parse_error.getLocation(), native_parse_error, 0, 0, 1,
                        parse_error.getMessage())) {
                    return 2;
                }
            }

            if (result.module) {
                for (const Luau::TypeError &type_error : result.module->errors) {
                    if (!checker->emit(type_error)) {
                        return 2;
                    }
                }
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_parse(Checker *checker, InstarSlice name, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!checker || !name_view) {
                return failure(error, "invalid parse request");
            }

            checker->frontend.parse(std::string(*name_view));

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_parse_diagnostics(Checker *checker, InstarSlice name, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!checker || !name_view) {
                return failure(error, "invalid parse diagnostic request");
            }

            const Luau::SourceModule *source = checker->frontend.getSourceModule(std::string(*name_view));

            if (!source) {
                return failure(error, "source unavailable after parse");
            }

            for (const Luau::ParseError &parse_error : source->parseErrors) {
                if (!checker->emit_diagnostic(
                        *name_view, parse_error.getLocation(), native_parse_error, 0, 0, 1, parse_error.getMessage())) {
                    return 2;
                }
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_ast(Checker *checker, InstarSlice name, InstarString *output_value, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (output_value) {
                *output_value = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!checker || !name_view || !output_value) {
                return failure(error, "invalid AST request");
            }

            const Luau::SourceModule *source = checker->frontend.getSourceModule(std::string(*name_view));

            if (!source || !source->root) {
                return failure(error, "source unavailable after parse");
            }

            output(output_value, Luau::toJson(source->root, source->commentLocations));

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_globals(Checker *checker, InstarItemCallback callback, void *context, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!checker || !callback) {
                return failure(error, "invalid global request");
            }

            for (const auto &[name, _] : checker->frontend.globals.globalScope->bindings) {
                if (!callback(context, slice(name.c_str()))) {
                    return 2;
                }
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_check(Checker *checker, InstarSlice name, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!checker || !name_view) {
                return failure(error, "invalid check request");
            }

            checker->check_result = checker->frontend.check(std::string(*name_view));

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_result(
        Checker *checker, InstarSlice name, uint8_t accumulate_nested, uint8_t for_autocomplete, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!checker || !name_view) {
                return failure(error, "invalid result request");
            }

            const std::optional<Luau::CheckResult> result = checker->frontend.getCheckResult(
                std::string(*name_view), accumulate_nested != 0, for_autocomplete != 0);

            if (!result) {
                return failure(error, "check result unavailable");
            }

            checker->check_result = *result;

            return checker->emit(*result, *name_view) ? 0 : 2;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_timeouts(Checker *checker, InstarItemCallback callback, void *context, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!checker || !callback) {
                return failure(error, "invalid timeout request");
            }

            if (!checker->check_result) {
                return failure(error, "check result unavailable");
            }

            for (const Luau::ModuleName &name : checker->check_result->timeoutHits) {
                if (!callback(context, slice(name))) {
                    return 2;
                }
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_modules(Checker *checker, InstarItemCallback callback, void *context, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!checker || !callback) {
                return failure(error, "invalid module request");
            }

            for (const auto &[name, _] : checker->frontend.sourceNodes) {
                if (!callback(context, slice(name))) {
                    return 2;
                }
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_attach_type_data(Checker *checker, InstarSlice name, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!checker || !name_view) {
                return failure(error, "invalid type data request");
            }

            Luau::SourceModule *source = checker->frontend.getSourceModule(std::string(*name_view));
            Luau::ModulePtr module = checker->frontend.moduleResolver.getModule(std::string(*name_view));

            if (!source || !module) {
                return failure(error, "checked module unavailable for type data");
            }

            Luau::attachTypeData(*source, *module);

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_checker_pretty_print_types(
        Checker *checker, InstarSlice name, InstarString *output_value, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (output_value) {
                *output_value = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!checker || !name_view || !output_value) {
                return failure(error, "invalid typed source request");
            }

            Luau::SourceModule *source = checker->frontend.getSourceModule(std::string(*name_view));

            if (!source || !source->root) {
                return failure(error, "checked module unavailable for typed source");
            }

            output(output_value, Luau::prettyPrintWithTypes(*source->root));

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    void *instar_state_new(InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            lua_State *state = luaL_newstate();

            if (!state) {
                failure(error, "cannot create Luau state");

                return nullptr;
            }

            return state;
        } catch (...) {
            exception_noexcept(error);

            return nullptr;
        }
    }

    void instar_state_destroy(void *state) {
        destroy_state(static_cast<lua_State *>(state));
    }

    int instar_state_openlibs(void *state, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!state) {
                return failure(error, "null Luau state");
            }

            luaL_openlibs(static_cast<lua_State *>(state));

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_state_get_global(void *state, InstarSlice name, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!state || !name_view) {
                return failure(error, "invalid Luau global request");
            }

            const std::string key(*name_view);
            lua_getglobal(static_cast<lua_State *>(state), key.c_str());

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_state_get_field(void *state, int index, InstarSlice name, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> name_view = view(name);

            if (!state || !name_view) {
                return failure(error, "invalid Luau field request");
            }

            const std::string key(*name_view);
            lua_getfield(static_cast<lua_State *>(state), index, key.c_str());

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_state_push_string(void *state, InstarSlice value, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> value_view = view(value);

            if (!state || !value_view) {
                return failure(error, "invalid Luau string request");
            }

            lua_pushlstring(static_cast<lua_State *>(state), value_view->data(), value_view->size());

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_state_call(
        void *state, int arguments, int results, int error_function, int *lua_status, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (!state || !lua_status) {
                return failure(error, "invalid Luau call request");
            }

            *lua_status = lua_pcall(static_cast<lua_State *>(state), arguments, results, error_function);

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_state_is_nil(void *state, int index, uint8_t *result, InstarString *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!state || !result) {
                return failure(error, "invalid Luau result request");
            }

            *result = uint8_t(lua_type(static_cast<lua_State *>(state), index) == LUA_TNIL);

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }

    int instar_state_to_string(
        void *state, int index, uint8_t *present, InstarString *output_value, InstarString *error) {

        try {
            if (error) {
                *error = {};
            }

            if (output_value) {
                *output_value = {};
            }

            if (!state || !present || !output_value) {
                return failure(error, "invalid Luau text request");
            }

            size_t length = 0;
            const char *value = lua_tolstring(static_cast<lua_State *>(state), index, &length);
            *present = uint8_t(value != nullptr);

            if (value) {
                output(output_value, std::string_view(value, length));
            }

            return 0;
        } catch (...) {
            return exception_noexcept(error);
        }
    }
}
