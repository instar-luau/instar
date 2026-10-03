#ifndef BRIDGE_HPP
#define BRIDGE_HPP

#include "Luau/Config.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "rust/cxx.h"

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_set>

namespace instar {
    struct Host;
    struct Items;
    struct Failure;
    struct FastFlag;
    struct RobloxClass;
    struct RobloxNode;
    enum class DiagnosticSeverity : uint32_t;
    class Operation;
    struct Checker;

    struct Configuration {
        Luau::Config value;
    };

    struct FileResolver final : Luau::FileResolver {
        explicit FileResolver(Checker *checker) : checker(checker) {}

        std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override;
        std::optional<Luau::ModuleInfo> resolveModule(const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override;
        std::string getHumanReadableModuleName(const Luau::ModuleName &name) const override { return name; }
        std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName &name) const override;

        Checker *checker;
        std::string origin;
    };

    struct ConfigurationResolver final : Luau::ConfigResolver {
        explicit ConfigurationResolver(Checker *checker) : checker(checker) {}

        const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override;

        Checker *checker;
    };

    struct Checker {
        explicit Checker(bool retain);

        void diagnostic(
            std::string_view path, const Luau::Location &location, DiagnosticSeverity severity, std::string_view message, std::string_view rule = {}, std::string_view related_path = {},
            const Luau::Location *related_location = nullptr, std::string_view related_message = {}
        );

        void emit(const Luau::TypeError &error, std::string_view checked);
        void emit(std::string_view path, const Luau::LintWarning &warning, bool error);

        FileResolver files;
        ConfigurationResolver configurations;
        Luau::Frontend frontend;
        std::unordered_set<std::string> definition_names;
        std::unordered_set<std::string> script_modules;
        bool roblox_classes_registered = false;
        bool roblox_tree_registered = false;
        bool globals_frozen = false;

      private:
        friend class Operation;
        friend struct FileResolver;
        friend struct ConfigurationResolver;
        Host *host = nullptr;
    };

    std::unique_ptr<Configuration> configuration_create(rust::Slice<const uint8_t> source, Failure &failure) noexcept;
    std::unique_ptr<Checker> checker_create(bool retain, Failure &failure) noexcept;
    Failure fast_flags(rust::Vec<FastFlag> &output) noexcept;
    Failure set_fast_flag(rust::Str name, bool boolean, bool bool_value, int32_t int_value) noexcept;
    Failure checker_mark_dirty(Checker &checker, rust::Str name) noexcept;
    Failure checker_clear_sources(Checker &checker) noexcept;
    Failure checker_freeze(Checker &checker) noexcept;
    Failure checker_load_definition(Checker &checker, Host &host, rust::Slice<const uint8_t> source, rust::Str package) noexcept;
    Failure checker_register_roblox_classes(Checker &checker, rust::Slice<const RobloxClass> classes) noexcept;
    Failure checker_register_roblox_tree(Checker &checker, rust::Slice<const RobloxNode> nodes) noexcept;
    Failure checker_parse(Checker &checker, Host &host, rust::Str name) noexcept;
    Failure checker_parse_diagnostics(Checker &checker, Host &host, rust::Str name) noexcept;
    Failure checker_prepare(Checker &checker, Host &host, rust::Str name, rust::Vec<rust::String> &timeouts) noexcept;
    Failure checker_check(Checker &checker, Host &host, rust::Str name) noexcept;
    Failure checker_lint(Checker &checker, Host &host, rust::Str name) noexcept;
    Failure checker_globals(const Checker &checker, Items &output) noexcept;
    Failure checker_modules(const Checker &checker, Items &output) noexcept;
    Failure checker_attach_type_data(Checker &checker, rust::Str name) noexcept;
} // namespace instar

#endif
