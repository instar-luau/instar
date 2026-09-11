#include "Luau/AstQuery.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Config.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Flags.h"
#include "Luau/Frontend.h"
#include "Luau/Parser.h"
#include "Luau/ToString.h"
#include "Luau/TypeArena.h"

#include <map>
#include <mutex>
#include <set>
#include <string>
#include <type_traits>
#include <vector>

extern "C" {
struct Bytes {
  const char *data;
  size_t size;
};

using Read = Bytes (*)(void *, Bytes);
using Resolve = Bytes (*)(void *, Bytes, Bytes);
using Configuration = Bytes (*)(void *, Bytes, size_t);
using Report = void (*)(void *, Bytes, Bytes, unsigned, unsigned, bool);
using Annotate = void (*)(void *, Bytes, Bytes);
using Alias = void (*)(void *, Bytes, Bytes);
}

static Bytes bytes(const std::string &value) {
  return {value.data(), value.size()};
}

static std::string text(Bytes value) {
  return value.size ? std::string(value.data, value.size) : std::string{};
}

void instar_initialize() {
  static std::once_flag flags;
  std::call_once(flags, setLuauFlagsDefault);
}

struct Files : Luau::FileResolver {
  void *context;
  Read read;
  Resolve resolve;

  std::optional<Luau::SourceCode>
  readSource(const Luau::ModuleName &name) override {
    Bytes source = read(context, bytes(name));

    if (!source.data)
      return std::nullopt;

    return Luau::SourceCode{text(source), Luau::SourceCode::Module};
  }

  std::optional<Luau::ModuleInfo>
  resolveModule(const Luau::ModuleInfo *from, Luau::AstExpr *expression,
                const Luau::TypeCheckLimits &) override {
    auto *literal = expression->as<Luau::AstExprConstantString>();

    if (!from || !literal)
      return std::nullopt;

    Bytes target = resolve(context, bytes(from->name),
                           {literal->value.data, literal->value.size});

    if (!target.data)
      return std::nullopt;

    return Luau::ModuleInfo{text(target)};
  }
};

static Luau::ConfigOptions configurationOptions() {
  Luau::ConfigOptions options;
  options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};

  return options;
}

struct Configurations : Luau::ConfigResolver {
  void *context;
  Configuration configuration;
  Report report;
  bool strict;
  mutable std::map<std::string, Luau::Config> cache;

  const Luau::Config &getConfig(const Luau::ModuleName &name,
                                const Luau::TypeCheckLimits &) const override {
    auto [iterator, inserted] = cache.try_emplace(name);

    if (!inserted)
      return iterator->second;

    auto &result = iterator->second;

    if (strict)
      result.mode = Luau::Mode::Strict;

    for (size_t index = 0;; ++index) {
      Bytes contents = configuration(context, bytes(name), index);

      if (!contents.data)
        break;

      if (auto error =
              Luau::parseConfig(text(contents), result, configurationOptions()))
        report(context, bytes(name), bytes(*error), 0, 0, true);
    }

    return result;
  }
};

extern "C" void instar_aliases(Bytes source, void *context, Alias alias,
                               Report report) noexcept {
  try {
    instar_initialize();
    Luau::Config configuration;

    if (auto error = Luau::parseConfig(text(source), configuration,
                                       configurationOptions())) {
      report(context, {}, bytes(*error), 0, 0, true);

      return;
    }

    for (const auto &entry : configuration.aliases)
      alias(context, bytes(entry.first), bytes(entry.second.value));
  } catch (const std::exception &error) {
    report(context, {}, bytes(error.what()), 0, 0, true);
  } catch (...) {
    report(context, {}, bytes("native configuration failure"), 0, 0, true);
  }
}

struct Annotations : Luau::AstVisitor {
  Luau::Module &module;
  std::vector<size_t> lines{0};
  std::map<size_t, std::string> insertions;
  Luau::ToStringOptions options;

  Annotations(Luau::Module &module, const std::string &source)
      : module(module) {
    for (size_t index = 0; index < source.size(); ++index)
      if (source[index] == '\n')
        lines.push_back(index + 1);
  }

  template <typename Type>
  void insert(Luau::Position position, Type type, const Luau::ScopePtr &scope) {
    options.scope = scope;
    auto result = Luau::toStringDetailed(type, options);

    if (result.invalid || result.error || result.cycle || result.truncated)
      return;

    std::string annotation = result.name;

    if constexpr (std::is_same_v<Type, Luau::TypePackId>)
      if (annotation != "()")
        annotation = "(" + annotation + ")";

    std::string declaration = "type Annotation = ";

    if constexpr (std::is_same_v<Type, Luau::TypePackId>)
      declaration += "() -> ";

    declaration += annotation;

    Luau::Allocator allocator;
    Luau::AstNameTable names(allocator);

    auto parsed = Luau::Parser::parse(declaration.data(), declaration.size(),
                                      names, allocator);

    if (!parsed.errors.empty())
      return;

    insertions.emplace(lines.at(position.line) + position.column,
                       ": " + annotation);
  }

  void local(Luau::AstLocal *value) {
    if (value->annotation)
      return;

    if (auto scope = Luau::findScopeAtPosition(module, value->location.begin))
      if (auto type = scope->lookup(value))
        insert(value->location.end, *type, scope);
  }

  bool visit(Luau::AstStatLocal *statement) override {
    for (auto *value : statement->vars)
      local(value);

    return true;
  }

  bool visit(Luau::AstStatFor *statement) override {
    local(statement->var);

    return true;
  }

  bool visit(Luau::AstStatForIn *statement) override {
    for (auto *value : statement->vars)
      local(value);

    return true;
  }

  bool visit(Luau::AstExprFunction *function) override {
    if (function->argLocation && function->generics.size == 0 &&
        function->genericPacks.size == 0)
      if (auto type = module.astTypes.find(function))
        if (auto signature =
                Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
          std::string generics;

          auto generic = [&](const auto &type) {
            if (!generics.empty())
              generics += ", ";

            generics += Luau::toString(type, options);
          };

          for (auto type : signature->generics)
            generic(type);

          for (auto type : signature->genericPacks)
            generic(type);

          if (!generics.empty())
            insertions.emplace(lines.at(function->argLocation->begin.line) +
                                   function->argLocation->begin.column,
                               "<" + generics + ">");
        }

    for (auto *argument : function->args)
      local(argument);

    if (!function->returnAnnotation && function->argLocation)
      if (auto scope =
              Luau::findScopeAtPosition(module, function->body->location.begin))
        insert(function->argLocation->end, scope->returnType, scope);

    return true;
  }
};

extern "C" void instar_analyze(void *context, Read read, Resolve resolve,
                               Configuration configuration, Report report,
                               Annotate annotate, const Bytes *modules,
                               size_t count, const Bytes *definitions,
                               size_t definitionCount, bool strict,
                               bool oldSolver, bool annotations) noexcept {
  static std::mutex mutex;

  try {
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
    options.retainFullTypeGraphs = true;

    Luau::Frontend frontend(oldSolver ? Luau::SolverMode::Old
                                      : Luau::SolverMode::New,
                            &files, &configurations, options);

    Luau::registerBuiltinGlobals(frontend, frontend.globals);

    auto diagnostic = [&](const std::string &module,
                          const Luau::Location &location,
                          const std::string &message, bool error) {
      report(context, bytes(module), bytes(message), location.begin.line,
             location.begin.column, error);
    };

    auto typeDiagnostic = [&](const std::string &name,
                              const Luau::TypeError &error) {
      std::string message;

      if (const auto *syntax = Luau::get_if<Luau::SyntaxError>(&error.data))
        message = "SyntaxError: " + syntax->message;
      else
        message = "TypeError: " +
                  Luau::toString(error, Luau::TypeErrorToStringOptions{&files});

      diagnostic(name, error.location, message, true);
    };

    std::set<std::string> loaded;

    for (size_t index = 0; index < definitionCount; ++index) {
      std::string name = text(definitions[index]);

      if (!loaded.insert(name).second)
        continue;

      Bytes source = read(context, bytes(name));

      if (!source.data) {
        report(context, bytes(name), bytes("definition source unavailable"), 0,
               0, true);

        continue;
      }

      auto result = frontend.loadDefinitionFile(frontend.globals,
                                                frontend.globals.globalScope,
                                                text(source), name, true);

      for (const auto &error : result.parseResult.errors)
        diagnostic(name, error.getLocation(),
                   "SyntaxError: " + error.getMessage(), true);

      if (result.module)
        for (const auto &error : result.module->errors)
          typeDiagnostic(name, error);
    }

    Luau::freeze(frontend.globals.globalTypes);

    for (size_t index = 0; index < count; ++index)
      if (!loaded.count(text(modules[index])))
        frontend.queueModuleCheck(text(modules[index]));

    for (const std::string &name : frontend.checkQueuedModules()) {
      auto checked = frontend.getCheckResult(name, false);

      if (!checked) {
        report(context, bytes(name),
               bytes("native analysis result unavailable"), 0, 0, true);

        continue;
      }

      const auto &result = *checked;

      for (const auto &error : result.errors)
        typeDiagnostic(error.moduleName, error);

      for (const auto &warning : result.lintResult.errors)
        diagnostic(name, warning.location,
                   std::string(Luau::LintWarning::getName(warning.code)) +
                       ": " + warning.text,
                   true);

      for (const auto &warning : result.lintResult.warnings)
        diagnostic(name, warning.location,
                   std::string(Luau::LintWarning::getName(warning.code)) +
                       ": " + warning.text,
                   false);

      if (annotations) {
        auto *source = frontend.getSourceModule(name);
        auto module = frontend.moduleResolver.getModule(name);

        if (source && module) {
          std::string contents = text(read(context, bytes(name)));
          Annotations annotation(*module, contents);
          source->root->visit(&annotation);

          for (auto iterator = annotation.insertions.rbegin();
               iterator != annotation.insertions.rend(); ++iterator)
            contents.insert(iterator->first, iterator->second);

          annotate(context, bytes(name), bytes(contents));
        }
      }
    }
  } catch (const std::exception &error) {
    report(context, {}, bytes(error.what()), 0, 0, true);
  } catch (...) {
    report(context, {}, bytes("native analysis failure"), 0, 0, true);
  }
}
