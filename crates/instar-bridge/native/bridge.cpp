#include "Luau/BuiltinDefinitions.h"
#include "Luau/Common.h"
#include "Luau/Config.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "Luau/Linter.h"
#include "Luau/Module.h"
#include "Luau/TypeAttach.h"
#include "bridge.hpp"
#include "editor.hpp"
#include "roblox.hpp"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <memory>
#include <new>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_set>
#include <utility>
#include <vector>

LUAU_FASTINT(LuauTarjanChildLimit)

namespace {

    struct CallbackFailure final : std::exception {
        const char *what() const noexcept override { return "native callback failed"; }
    };

    std::optional<std::string_view> view(Text value) {
        if (!value.data && value.length != 0) {
            return std::nullopt;
        }

        return std::string_view(value.data ? reinterpret_cast<const char *>(value.data) : "", value.length);
    }

    Text text(std::string_view value) {
        return {reinterpret_cast<const uint8_t *>(value.data()), value.size()};
    }

    void write(String *destination, std::string_view value) {
        destination->data = nullptr;
        destination->length = 0;

        if (value.empty()) {
            return;
        }

        auto *data = static_cast<uint8_t *>(std::malloc(value.size()));

        if (!data) {
            throw std::bad_alloc();
        }

        std::memcpy(data, value.data(), value.size());
        destination->data = data;
        destination->length = value.size();
    }

    int32_t failure(String *error, std::string_view message) {
        if (error) {
            write(error, message);
        }

        return StatusFailure;
    }

    int32_t exception_failure(String *error) noexcept {
        try {
            throw;
        } catch (const std::exception &value) {
            if (error) {
                error->data = nullptr;
                error->length = 0;
                const size_t length = std::strlen(value.what());
                auto *data = static_cast<uint8_t *>(std::malloc(length));

                if (data) {
                    std::memcpy(data, value.what(), length);
                    error->data = data;
                    error->length = length;
                }
            }
        } catch (...) {
            if (error) {
                error->data = nullptr;
                error->length = 0;
            }
        }

        return StatusFailure;
    }

    struct Configuration {
        Luau::Config value;
    };

    struct Checker;

    struct FileResolver final : Luau::FileResolver {
        explicit FileResolver(Checker *checker) : checker(checker) {}

        std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override;
        std::optional<Luau::ModuleInfo> resolveModule(const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override;
        std::string getHumanReadableModuleName(const Luau::ModuleName &name) const override { return name; }
        std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName &name) const override;

        Checker *checker;
    };

    struct ConfigurationResolver final : Luau::ConfigResolver {
        explicit ConfigurationResolver(Checker *checker) : checker(checker) {}

        const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override;

        Checker *checker;
    };

    struct Checker {
        Checker(const BridgeCallbacks &callbacks, void *context, const FrontendOptions &options)
            : callbacks(callbacks), context(context), files(this), configurations(this), frontend(
                                                                                             Luau::SolverMode::New, &files, &configurations,
                                                                                             Luau::FrontendOptions{
                                                                                                 options.retain_full_type_graphs != 0,
                                                                                                 options.for_autocomplete != 0,
                                                                                                 options.run_lint_checks != 0,
                                                                                             }
                                                                                         ) {
            Luau::registerBuiltinGlobals(frontend, frontend.globals, options.for_autocomplete != 0);
        }

        bool diagnostic(
            std::string_view path, const Luau::Location &location, DiagnosticSeverity severity, std::string_view message, std::string_view related_path = {},
            const Luau::Location *related_location = nullptr, std::string_view related_message = {}
        ) {
            Diagnostic value{};
            value.path = text(path);
            value.location = {location.begin.line, location.begin.column, location.end.line, location.end.column};
            value.severity = severity;
            value.message = text(message);
            value.has_related = related_location != nullptr;

            if (related_location) {
                value.related.path = text(related_path);

                value.related.location = {
                    related_location->begin.line,
                    related_location->begin.column,
                    related_location->end.line,
                    related_location->end.column,
                };

                value.related.message = text(related_message);
            }

            return callbacks.diagnostic(context, &value) != 0;
        }

        bool emit(const Luau::TypeError &error) {
            const std::string message = Luau::toString(error, Luau::TypeErrorToStringOptions{&files});
            const Luau::DuplicateTypeDefinition *duplicate = Luau::get<Luau::DuplicateTypeDefinition>(error);

            if (duplicate && duplicate->previousLocation) {
                return diagnostic(error.moduleName, error.location, DiagnosticError, message, error.moduleName, &*duplicate->previousLocation, "previous definition");
            }

            return diagnostic(error.moduleName, error.location, DiagnosticError, message);
        }

        bool emit(std::string_view path, const Luau::LintWarning &warning, bool error) {
            const std::string message = std::string(Luau::LintWarning::getName(warning.code)) + ": " + warning.text;

            return diagnostic(path, warning.location, error ? DiagnosticError : DiagnosticWarning, message);
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

        static int32_t timeouts(const Luau::CheckResult &result, ItemCallback callback, void *context) {
            for (const Luau::ModuleName &name : result.timeoutHits) {
                if (!callback(context, text(name))) {
                    return StatusCallbackFailure;
                }
            }

            return StatusSuccess;
        }

        BridgeCallbacks callbacks;
        void *context;
        FileResolver files;
        ConfigurationResolver configurations;
        Luau::Frontend frontend;
        std::unordered_set<std::string> definition_names;
        std::unordered_set<std::string> script_modules;
        bool roblox_classes_registered = false;
        bool roblox_tree_registered = false;
        bool globals_frozen = false;
    };

    std::optional<std::string> FileResolver::getEnvironmentForModule(const Luau::ModuleName &name) const {
        return checker->script_modules.count(name) ? std::optional<std::string>(name) : std::nullopt;
    }

    std::optional<Luau::SourceCode> FileResolver::readSource(const Luau::ModuleName &name) {
        SourceResult result{};

        if (!checker->callbacks.source(checker->context, text(name), &result)) {
            throw CallbackFailure{};
        }

        const std::optional<std::string_view> source = view(result.source);

        if (!source) {
            throw CallbackFailure{};
        }

        if (result.kind == SourceUnknown) {
            return std::nullopt;
        }

        if (result.kind != SourceScript && result.kind != SourceModule) {
            throw CallbackFailure{};
        }

        return Luau::SourceCode{std::string(*source), static_cast<Luau::SourceCode::Type>(result.kind)};
    }

    std::optional<Luau::ModuleInfo> FileResolver::resolveModule(const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) {
        if (!context || !expression) {
            return std::nullopt;
        }

        ResolveRequest request{
            text(context->name),
            uint8_t(context->optional),
            {
                expression->location.begin.line,
                expression->location.begin.column,
                expression->location.end.line,
                expression->location.end.column,
            },
        };

        ResolveResult result{};

        if (!checker->callbacks.resolve(checker->context, &request, &result)) {
            throw CallbackFailure{};
        }

        if (!result.present) {
            return std::nullopt;
        }

        const std::optional<std::string_view> path = view(result.path);

        if (!path) {
            throw CallbackFailure{};
        }

        return Luau::ModuleInfo{std::string(*path), context->optional};
    }

    const Luau::Config &ConfigurationResolver::getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const {
        const void *configuration = nullptr;

        if (!checker->callbacks.configuration(checker->context, text(name), &configuration) || !configuration) {
            throw CallbackFailure{};
        }

        return static_cast<const Configuration *>(configuration)->value;
    }
} // namespace

template <typename Function, typename... Arguments> int32_t call_editor(void *handle, String *error, Function function, Arguments &&...arguments) {
    try {
        if (error) {
            *error = {};
        }

        auto *checker = static_cast<Checker *>(handle);

        if (!checker) {
            return failure(error, "null checker");
        }

        return function(checker->frontend, std::forward<Arguments>(arguments)...);
    } catch (...) {
        return exception_failure(error);
    }
}

extern "C" {
    void string_destroy(String value) {
        std::free(value.data);
    }

    void *configuration_create(Text source, String *error) {
        try {
            if (error) {
                *error = {};
            }

            const std::optional<std::string_view> value = view(source);

            if (!value) {
                failure(error, "invalid configuration source");

                return nullptr;
            }

            Luau::Config configuration;

            if (const std::optional<std::string> message = Luau::parseConfig(std::string(*value), configuration)) {
                failure(error, *message);

                return nullptr;
            }

            return new Configuration{std::move(configuration)};
        } catch (...) {
            exception_failure(error);

            return nullptr;
        }
    }

    void configuration_destroy(void *configuration) {
        delete static_cast<Configuration *>(configuration);
    }

    void *checker_create(const BridgeCallbacks *callbacks, void *context, const FrontendOptions *options, String *error) {
        try {
            if (error) {
                *error = {};
            }

            if (!callbacks || !callbacks->source || !callbacks->configuration || !callbacks->resolve || !callbacks->diagnostic || !options) {
                failure(error, "invalid checker callbacks");

                return nullptr;
            }

            // Modern Roblox definitions exceed Luau's default 10,000-child limit.
            FInt::LuauTarjanChildLimit.value = 12'500;

            return new Checker(*callbacks, context, *options);
        } catch (...) {
            exception_failure(error);

            return nullptr;
        }
    }

    void checker_destroy(void *handle) {
        delete static_cast<Checker *>(handle);
    }

    int32_t checker_set_context(void *handle, void *context, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);

            if (!checker) {
                return failure(error, "null checker");
            }

            checker->context = context;

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_mark_dirty(void *handle, Text name, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);
            const std::optional<std::string_view> value = view(name);

            if (!checker || !value) {
                return failure(error, "invalid dirty module request");
            }

            if (checker->definition_names.count(std::string(*value))) {
                return failure(error, "definition changes require a new checker");
            }

            checker->frontend.markDirty(std::string(*value));

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_clear_sources(void *handle, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);

            if (!checker) {
                return failure(error, "null checker");
            }

            std::vector<Luau::ModuleName> names;
            names.reserve(checker->frontend.sourceNodes.size());

            for (const auto &[name, _] : checker->frontend.sourceNodes) {
                if (!checker->definition_names.count(name)) {
                    names.push_back(name);
                }
            }

            checker->frontend.clearModules(names);

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_register_roblox_classes(void *handle, const RobloxClass *classes, size_t class_count, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);

            if (!checker || (class_count != 0 && !classes)) {
                return failure(error, "invalid Roblox registration request");
            }

            if (checker->globals_frozen || checker->roblox_classes_registered || !checker->frontend.sourceNodes.empty()) {
                return failure(error, "global changes require a new checker");
            }

            instar::register_roblox_magic(checker->frontend.globals, classes, class_count);
            checker->roblox_classes_registered = true;

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_register_roblox_tree(void *handle, const RobloxNode *nodes, size_t node_count, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);

            if (!checker || !nodes || node_count == 0) {
                return failure(error, "invalid Roblox hierarchy request");
            }

            if (!checker->roblox_classes_registered) {
                return failure(error, "register Roblox classes before the hierarchy");
            }

            if (checker->globals_frozen || checker->roblox_tree_registered || !checker->frontend.sourceNodes.empty()) {
                return failure(error, "hierarchy changes require a new checker");
            }

            checker->script_modules = instar::register_roblox_tree(checker->frontend, nodes, node_count);
            checker->roblox_tree_registered = true;

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_freeze(void *handle, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);

            if (!checker) {
                return failure(error, "null checker");
            }

            Luau::freeze(checker->frontend.globals.globalTypes);
            checker->globals_frozen = true;

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_load_definition(void *handle, Text source, Text package_name, const DefinitionOptions *options, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);
            const std::optional<std::string_view> source_view = view(source);
            const std::optional<std::string_view> package_view = view(package_name);

            if (!checker || !source_view || !package_view || !options) {
                return failure(error, "invalid definition request");
            }

            const std::string name(*package_view);

            if (checker->globals_frozen || checker->roblox_classes_registered || !checker->frontend.sourceNodes.empty() || checker->frontend.sourceModules.count(name)) {
                return failure(error, "definition changes require a new checker");
            }

            Luau::LoadDefinitionFileResult result = checker->frontend.loadDefinitionFile(
                checker->frontend.globals,
                checker->frontend.globals.globalScope,
                *source_view,
                name,
                options->capture_comments != 0,
                options->type_check_for_autocomplete != 0
            );

            for (const Luau::ParseError &parse_error : result.parseResult.errors) {
                if (!checker->diagnostic(name, parse_error.getLocation(), DiagnosticError, parse_error.getMessage())) {
                    return StatusCallbackFailure;
                }
            }

            if (result.module) {
                for (const Luau::TypeError &type_error : result.module->errors) {
                    if (!checker->emit(type_error)) {
                        return StatusCallbackFailure;
                    }
                }
            }

            if (!result.success) {
                return failure(error, "definition loading failed");
            }

            checker->frontend.sourceModules[name] = std::make_shared<Luau::SourceModule>(std::move(result.sourceModule));
            checker->frontend.moduleResolver.setModule(name, result.module);
            checker->frontend.moduleResolverForAutocomplete.setModule(name, std::move(result.module));
            checker->definition_names.insert(name);

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_parse(void *handle, Text name, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);
            const std::optional<std::string_view> value = view(name);

            if (!checker || !value) {
                return failure(error, "invalid parse request");
            }

            if (checker->definition_names.find(std::string(*value)) == checker->definition_names.end()) {
                checker->frontend.parse(std::string(*value));
            }

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_parse_diagnostics(void *handle, Text name, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);
            const std::optional<std::string_view> value = view(name);

            if (!checker || !value) {
                return failure(error, "invalid parse diagnostic request");
            }

            const Luau::SourceModule *source = checker->frontend.getSourceModule(std::string(*value));

            if (!source) {
                return failure(error, "source unavailable after parse");
            }

            for (const Luau::ParseError &parse_error : source->parseErrors) {
                if (!checker->diagnostic(*value, parse_error.getLocation(), DiagnosticError, parse_error.getMessage())) {
                    return StatusCallbackFailure;
                }
            }

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_check(void *handle, Text name, ItemCallback timeout_callback, void *timeout_context, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);
            const std::optional<std::string_view> value = view(name);

            if (!checker || !value || !timeout_callback) {
                return failure(error, "invalid check request");
            }

            if (checker->definition_names.count(std::string(*value))) {
                return StatusSuccess;
            }

            const auto result = checker->frontend.check(std::string(*value));

            return Checker::timeouts(result, timeout_callback, timeout_context);
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t
    checker_result(void *handle, Text name, uint8_t accumulate_nested, uint8_t for_autocomplete, ItemCallback timeout_callback, void *timeout_context, String *error) {

        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);
            const std::optional<std::string_view> value = view(name);

            if (!checker || !value || !timeout_callback) {
                return failure(error, "invalid result request");
            }

            if (checker->definition_names.find(std::string(*value)) != checker->definition_names.end()) {
                return StatusSuccess;
            }

            auto result = checker->frontend.getCheckResult(std::string(*value), accumulate_nested != 0, for_autocomplete != 0);

            if (!result) {
                return failure(error, "check result unavailable");
            }

            if (!checker->emit(*result, *value)) {
                return StatusCallbackFailure;
            }

            return Checker::timeouts(*result, timeout_callback, timeout_context);
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_globals(void *handle, ItemCallback callback, void *context, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);

            if (!checker || !callback) {
                return failure(error, "invalid global request");
            }

            for (const auto &[name, _] : checker->frontend.globals.globalScope->bindings) {
                if (!callback(context, text(name.c_str()))) {
                    return StatusCallbackFailure;
                }
            }

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_modules(void *handle, ItemCallback callback, void *context, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);

            if (!checker || !callback) {
                return failure(error, "invalid module request");
            }

            for (const auto &[name, _] : checker->frontend.sourceNodes) {
                if (!callback(context, text(name))) {
                    return StatusCallbackFailure;
                }
            }

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t checker_attach_type_data(void *handle, Text name, String *error) {
        try {
            if (error) {
                *error = {};
            }

            auto *checker = static_cast<Checker *>(handle);
            const std::optional<std::string_view> value = view(name);

            if (!checker || !value) {
                return failure(error, "invalid type data request");
            }

            Luau::SourceModule *source = checker->frontend.getSourceModule(std::string(*value));
            Luau::ModulePtr module = checker->frontend.moduleResolver.getModule(std::string(*value));

            if (!source || !module) {
                return failure(error, "checked module unavailable for type data");
            }

            Luau::attachTypeData(*source, *module);

            return StatusSuccess;
        } catch (...) {
            return exception_failure(error);
        }
    }

    int32_t editor_hover(void *handle, Text name, uint32_t line, uint32_t column, HoverCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_hover, name, line, column, callback, context);
    }

    int32_t editor_completion(void *handle, Text name, uint32_t line, uint32_t column, CompletionCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_completion, name, line, column, callback, context);
    }

    int32_t editor_signature_help(void *handle, Text name, uint32_t line, uint32_t column, SignatureCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_signature_help, name, line, column, callback, context);
    }

    int32_t editor_type_hints(void *handle, Text name, HintCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_type_hints, name, callback, context);
    }

    int32_t editor_semantic_tokens(void *handle, Text name, TokenCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_semantic_tokens, name, callback, context);
    }

    int32_t editor_definition(void *handle, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_definition, name, line, column, callback, context);
    }

    int32_t editor_declaration(void *handle, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_declaration, name, line, column, callback, context);
    }

    int32_t editor_type_definition(void *handle, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_type_definition, name, line, column, callback, context);
    }

    int32_t editor_references(void *handle, Text name, uint32_t line, uint32_t column, ReferenceCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_references, name, line, column, callback, context);
    }

    int32_t editor_prepare(void *handle, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_prepare, name, line, column, callback, context);
    }

    int32_t editor_local_references(void *handle, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context, String *error) {
        return call_editor(handle, error, instar::editor_local_references, name, line, column, callback, context);
    }
}
