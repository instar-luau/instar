#include "Luau/BuiltinDefinitions.h"
#include "Luau/Config.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Flags.h"
#include "Luau/Frontend.h"
#include "Luau/PrettyPrinter.h"
#include "Luau/TypeAttach.h"
#include "Luau/TypeArena.h"

#include <map>
#include <mutex>
#include <string>

extern "C"
{
    struct Bytes
    {
        const char* data;
        size_t size;
    };

    using Read = Bytes (*)(void*, Bytes);
    using Resolve = Bytes (*)(void*, Bytes, Bytes);
    using Configuration = Bytes (*)(void*, Bytes, size_t);
    using Report = void (*)(void*, Bytes, Bytes, unsigned, unsigned, bool);
    using Annotate = void (*)(void*, Bytes, Bytes);
    using Alias = void (*)(void*, Bytes, Bytes);
}

static Bytes bytes(const std::string& value)
{
    return {value.data(), value.size()};
}

static std::string text(Bytes value)
{
    return value.size ? std::string(value.data, value.size) : std::string{};
}

void instar_initialize()
{
    static std::once_flag flags;
    std::call_once(flags, setLuauFlagsDefault);
}

struct Files : Luau::FileResolver
{
    void* context;
    Read read;
    Resolve resolve;

    std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override
    {
        Bytes source = read(context, bytes(name));

        if (!source.data)
            return std::nullopt;

        return Luau::SourceCode{text(source), Luau::SourceCode::Module};
    }

    std::optional<Luau::ModuleInfo> resolveModule(const Luau::ModuleInfo* from, Luau::AstExpr* expression, const Luau::TypeCheckLimits&) override
    {
        auto* literal = expression->as<Luau::AstExprConstantString>();

        if (!from || !literal)
            return std::nullopt;

        Bytes target = resolve(context, bytes(from->name), {literal->value.data, literal->value.size});

        if (!target.data)
            return std::nullopt;

        return Luau::ModuleInfo{text(target)};
    }
};

static Luau::ConfigOptions configurationOptions()
{
    Luau::ConfigOptions options;
    options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};

    return options;
}

struct Configurations : Luau::ConfigResolver
{
    void* context;
    Configuration configuration;
    Report report;
    bool strict;
    mutable std::map<std::string, Luau::Config> cache;

    const Luau::Config& getConfig(const Luau::ModuleName& name, const Luau::TypeCheckLimits&) const override
    {
        auto [iterator, inserted] = cache.try_emplace(name);

        if (!inserted)
            return iterator->second;

        auto& result = iterator->second;

        if (strict)
            result.mode = Luau::Mode::Strict;

        for (size_t index = 0;; ++index)
        {
            Bytes contents = configuration(context, bytes(name), index);

            if (!contents.data)
                break;

            if (auto error = Luau::parseConfig(text(contents), result, configurationOptions()))
                report(context, bytes(name), bytes(*error), 0, 0, true);
        }

        return result;
    }
};

extern "C" void instar_aliases(Bytes source, void* context, Alias alias, Report report) noexcept
{
    try
    {
        instar_initialize();
        Luau::Config configuration;

        if (auto error = Luau::parseConfig(text(source), configuration, configurationOptions()))
        {
            report(context, {}, bytes(*error), 0, 0, true);

            return;
        }

        for (const auto& entry : configuration.aliases)
            alias(context, bytes(entry.first), bytes(entry.second.value));
    }
    catch (const std::exception& error)
    {
        report(context, {}, bytes(error.what()), 0, 0, true);
    }
    catch (...)
    {
        report(context, {}, bytes("native configuration failure"), 0, 0, true);
    }
}

extern "C" void instar_analyze(
    void* context,
    Read read,
    Resolve resolve,
    Configuration configuration,
    Report report,
    Annotate annotate,
    const Bytes* modules,
    size_t count,
    bool strict,
    bool oldSolver,
    bool annotations
) noexcept
{
    static std::mutex mutex;

    try
    {
        std::lock_guard<std::mutex> lock(mutex);
        instar_initialize();
        Files files;
        files.context = context;
        files.read = read;
        files.resolve = resolve;
        Configurations configurations;
        configurations.context = context;
        configurations.configuration = configuration;
        configurations.report = report;
        configurations.strict = strict;
        Luau::FrontendOptions options;
        options.runLintChecks = true;
        options.retainFullTypeGraphs = annotations;
        Luau::Frontend frontend(oldSolver ? Luau::SolverMode::Old : Luau::SolverMode::New, &files, &configurations, options);
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
        Luau::freeze(frontend.globals.globalTypes);

        auto diagnostic = [&](const std::string& module, const Luau::Location& location, const std::string& message, bool error)
        {
            report(context, bytes(module), bytes(message), location.begin.line, location.begin.column, error);
        };

        for (size_t index = 0; index < count; ++index)
            frontend.queueModuleCheck(text(modules[index]));

        for (const std::string& name : frontend.checkQueuedModules())
        {
            auto checked = frontend.getCheckResult(name, false);

            if (!checked)
            {
                report(context, bytes(name), bytes("native analysis result unavailable"), 0, 0, true);
                continue;
            }

            const auto& result = *checked;

            for (const auto& error : result.errors)
            {
                std::string message;

                if (const auto* syntax = Luau::get_if<Luau::SyntaxError>(&error.data))
                    message = "SyntaxError: " + syntax->message;
                else
                    message = "TypeError: " + Luau::toString(error, Luau::TypeErrorToStringOptions{&files});

                diagnostic(error.moduleName, error.location, message, true);
            }

            for (const auto& warning : result.lintResult.errors)
                diagnostic(name, warning.location, std::string(Luau::LintWarning::getName(warning.code)) + ": " + warning.text, true);

            for (const auto& warning : result.lintResult.warnings)
                diagnostic(name, warning.location, std::string(Luau::LintWarning::getName(warning.code)) + ": " + warning.text, false);

            if (annotations)
            {
                auto* source = frontend.getSourceModule(name);
                auto module = frontend.moduleResolver.getModule(name);

                if (source && module)
                {
                    Luau::attachTypeData(*source, *module);
                    annotate(context, bytes(name), bytes(Luau::prettyPrintWithTypes(*source->root)));
                }
            }
        }
    }
    catch (const std::exception& error)
    {
        report(context, {}, bytes(error.what()), 0, 0, true);
    }
    catch (...)
    {
        report(context, {}, bytes("native analysis failure"), 0, 0, true);
    }
}
