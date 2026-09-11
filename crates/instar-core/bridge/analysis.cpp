#include "Luau/AstQuery.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Config.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Flags.h"
#include "Luau/Frontend.h"
#include "Luau/LuauConfig.h"
#include "Luau/Parser.h"
#include "Luau/ToString.h"
#include "Luau/TypeArena.h"
#include "roblox.hpp"

#include <algorithm>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <vector>

LUAU_FASTINT(LuauTarjanChildLimit)

extern "C" {
struct Bytes {
  const char *data;
  size_t size;
};

using Read = Bytes (*)(void *, Bytes);
using Resolve = Bytes (*)(void *, Bytes, Bytes, unsigned);
using Environment = Bytes (*)(void *, Bytes, Bytes, size_t);
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

  std::call_once(flags, [] {
    setLuauFlagsDefault();
    if (FInt::LuauTarjanChildLimit > 0 && FInt::LuauTarjanChildLimit < 15000)
      FInt::LuauTarjanChildLimit.value = 15000;
  });
}

struct Files : Luau::FileResolver {
  void *context;
  Read read;
  Resolve resolve;
  Environment environment;
  const std::vector<Luau::TypeId> *instances = nullptr;

  Bytes member(const std::string &from, const std::string &name) {
    if (!instances)
      return {};

    Bytes index = environment(context, bytes("script"), bytes(from), 0);

    if (!index.data)
      return {};

    auto type =
        Luau::get<Luau::ExternType>(instances->at(std::stoull(text(index))));

    if (name != "Parent" && type->parent &&
        Luau::lookupExternTypeProp(
            Luau::get<Luau::ExternType>(Luau::follow(*type->parent)), name))
      return {};

    return resolve(context, bytes(from), bytes(name), 2);
  }

  std::optional<Luau::SourceCode>
  readSource(const Luau::ModuleName &name) override {
    Bytes source = read(context, bytes(name));

    if (!source.data)
      return std::nullopt;

    auto contents = text(source);
    auto kind = text(environment(context, bytes("kind"), bytes(name), 0));

    return Luau::SourceCode{std::move(contents),
                            kind == "Script" || kind == "LocalScript"
                                ? Luau::SourceCode::Script
                                : Luau::SourceCode::Module};
  }

  std::optional<Luau::ModuleInfo>
  resolveModule(const Luau::ModuleInfo *from, Luau::AstExpr *expression,
                const Luau::TypeCheckLimits &) override {
    if (!from)
      return std::nullopt;

    Bytes target{};

    if (auto literal = expression->as<Luau::AstExprConstantString>())
      target = resolve(context, bytes(from->name),
                       {literal->value.data, literal->value.size}, 0);
    else if (auto global = expression->as<Luau::AstExprGlobal>())
      target =
          resolve(context, bytes(from->name), bytes(global->name.value), 1);
    else if (auto expressionMember = expression->as<Luau::AstExprIndexName>())
      target = member(from->name, expressionMember->index.value);
    else if (auto member = expression->as<Luau::AstExprIndexExpr>()) {
      if (auto literal = member->index->as<Luau::AstExprConstantString>())
        target = this->member(
            from->name, std::string(literal->value.data, literal->value.size));
    } else if (auto call = expression->as<Luau::AstExprCall>();
               call && call->self && call->args.size >= 1) {
      auto member = call->func->as<Luau::AstExprIndexName>();
      auto literal = call->args.data[0]->as<Luau::AstExprConstantString>();

      if (member && literal &&
          (member->index == "GetService" || member->index == "WaitForChild" ||
           member->index == "FindFirstChild"))
        target = resolve(context, bytes(from->name),
                         {literal->value.data, literal->value.size},
                         member->index == "GetService" ? 4 : 3);
    }

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

static bool isConfiguration(const std::string &path) {
  auto name = path.substr(path.find_last_of("/\\") + 1);

  return name == ".config.luau" || name == "config.luau";
}

struct Configurations : Luau::ConfigResolver {
  void *context;
  Configuration configuration;
  Report report;
  std::optional<Luau::Mode> mode;
  mutable std::map<std::string, Luau::Config> cache;

  const Luau::Config &getConfig(const Luau::ModuleName &name,
                                const Luau::TypeCheckLimits &) const override {
    auto [iterator, inserted] = cache.try_emplace(name);

    if (!inserted)
      return iterator->second;

    auto &result = iterator->second;

    for (size_t index = 0;; ++index) {
      Bytes contents = configuration(context, bytes(name), index);

      if (!contents.data)
        break;

      std::string payload = text(contents);
      size_t separator = payload.find('\0');

      if (separator == std::string::npos)
        throw std::runtime_error("configuration path unavailable");

      std::string path = payload.substr(0, separator);
      std::string source = payload.substr(separator + 1);
      std::optional<std::string> error;

      if (isConfiguration(path)) {
        Luau::InterruptCallbacks callbacks{};
        Luau::ConfigOptions::AliasOptions aliases;
        aliases.configLocation = path;
        aliases.overwriteAliases = true;

        error = Luau::extractLuauConfig(source, result, aliases,
                                        std::move(callbacks));
      } else {
        error = Luau::parseConfig(source, result, configurationOptions());
      }

      if (error)
        report(context, bytes(path), bytes(*error), 0, 0, true);
    }

    if (mode)
      result.mode = *mode;

    return result;
  }
};

extern "C" void instar_aliases(Bytes source, bool executable, void *context,
                               Alias alias, Report report) noexcept {
  try {
    instar_initialize();
    Luau::Config configuration;

    auto options = configurationOptions();

    auto error = executable
                     ? Luau::extractLuauConfig(text(source), configuration,
                                               options.aliasOptions, {})
                     : Luau::parseConfig(text(source), configuration, options);

    if (error) {
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

struct Definition {
  std::string path;
  std::string source;
};

struct Diagnostic {
  std::string path;
  Luau::Location location;
  std::string message;
  bool error;
};

struct Session {
  Files files;
  Configurations configurations;
  std::vector<Luau::TypeId> instances;
  std::vector<Diagnostic> diagnostics;
  std::vector<std::string> inputs;
  std::map<std::string, std::vector<std::string>> metadata;
  std::unique_ptr<Luau::Frontend> frontend;
};

static std::mutex analysisMutex;

extern "C" void instar_destroy(void *session) noexcept {
  std::lock_guard<std::mutex> lock(analysisMutex);
  delete static_cast<Session *>(session);
}

extern "C" void instar_analyze(void **handle, void *context, Read read, Resolve resolve,
                               Configuration configuration, Report report,
                               Annotate annotate, const Bytes *modules,
                               size_t count, const Bytes *definitions,
                               size_t definitionCount, Bytes configurationTypes,
                               Environment environment, Bytes mode,
                               bool oldSolver, bool annotations) noexcept {
  std::lock_guard<std::mutex> lock(analysisMutex);

  try {
    instar_initialize();

    auto metadata = [&](const std::string &category,
                        size_t index) -> std::optional<std::string> {
      Bytes value = environment(context, bytes(category), {}, index);

      return value.data ? std::optional<std::string>{text(value)} : std::nullopt;
    };

    std::map<std::string, std::vector<std::string>> values;

    for (const auto &category : {"enabled", "definitions"})
      if (auto value = metadata(category, 0))
        values[category].push_back(*value);

    for (const auto &category : {"enumeration", "class", "node"}) {
      auto &entries = values[category];

      for (size_t index = 0;; ++index) {
        auto value = metadata(category, index);

        if (!value)
          break;

        entries.push_back(*value);
      }
    }

    std::set<std::string> loaded;
    std::vector<Definition> sources;
    std::vector<std::string> inputs{oldSolver ? "old" : "new", text(configurationTypes)};

    for (size_t index = 0; index < definitionCount; ++index) {
      std::string name = text(definitions[index]);

      if (!loaded.insert(name).second)
        continue;

      Bytes source = read(context, bytes(name));

      if (!source.data)
        throw std::runtime_error("definition source unavailable: " + name);

      sources.push_back({name, text(source)});
      inputs.push_back(name);
      inputs.push_back(sources.back().source);
    }

    auto *previous = static_cast<Session *>(*handle);
    bool initializing = !previous || previous->inputs != inputs || previous->metadata != values;

    if (initializing) {
      delete previous;
      *handle = nullptr;
      auto replacement = std::make_unique<Session>();
      replacement->inputs = std::move(inputs);
      replacement->metadata = std::move(values);
      *handle = replacement.release();
    }

    auto &session = *static_cast<Session *>(*handle);
    auto &files = session.files;
    files.context = context;
    files.read = read;
    files.resolve = resolve;
    files.environment = environment;
    auto &configurations = session.configurations;
    configurations.cache.clear();
    configurations.mode.reset();
    configurations.context = context;
    configurations.configuration = configuration;
    configurations.report = report;

    if (mode.size) {
      Luau::Mode selected;

      if (auto error = Luau::parseModeString(selected, text(mode)))
        throw std::runtime_error(*error);

      configurations.mode = selected;
    }

    if (initializing) {
      Luau::FrontendOptions options;
      options.runLintChecks = true;
      options.retainFullTypeGraphs = true;

      session.frontend = std::make_unique<Luau::Frontend>(
          oldSolver ? Luau::SolverMode::Old : Luau::SolverMode::New,
          &files, &configurations, options);
    }

    auto &frontend = *session.frontend;
    frontend.clear();
    frontend.clearStats();

    auto diagnostic = [&](const std::string &module,
                          const Luau::Location &location,
                          const std::string &message, bool error) {
      if (initializing)
        session.diagnostics.push_back({module, location, message, error});

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

    bool roblox = session.metadata.count("enabled") != 0;

    if (initializing) {
      Luau::registerBuiltinGlobals(frontend, frontend.globals);
      std::string combined;
      std::vector<std::pair<unsigned, std::string>> origins;
      unsigned lines = 0;
      auto bundle = metadata("definitions", 0);

      auto append = [&](const std::string &name, const std::string &source) {
        origins.emplace_back(lines, name);
        combined += source + "\n";
        lines += unsigned(std::count(source.begin(), source.end(), '\n')) + 1;
      };

      if (!bundle)
        for (const auto &definition : sources) {
          const auto &name = definition.path;

          auto result = frontend.loadDefinitionFile(frontend.globals,
                                                    frontend.globals.globalScope,
                                                    definition.source, name, true);

          for (const auto &error : result.parseResult.errors)
            diagnostic(name, error.getLocation(),
                       "SyntaxError: " + error.getMessage(), true);

          if (result.module)
            for (const auto &error : result.module->errors)
              typeDiagnostic(name, error);
        }

      if (bundle) {
        std::string name = "roblox";

        append(name, *bundle);

        for (const auto &definition : sources)
          append(definition.path, definition.source);

        auto result = frontend.loadDefinitionFile(frontend.globals,
                                                  frontend.globals.globalScope,
                                                  combined, "@roblox", false);

        auto origin = [&](Luau::Location &location) {
          auto iterator = origins.rbegin();

          while (iterator != origins.rend() &&
                 iterator->first > location.begin.line)
            ++iterator;

          if (iterator == origins.rend())
            return name;

          location.begin.line -= iterator->first;
          location.end.line -= iterator->first;

          if (iterator->second == name)
            location = {};

          return iterator->second;
        };

        for (const auto &error : result.parseResult.errors) {
          auto location = error.getLocation();
          auto path = origin(location);
          diagnostic(path, location, "SyntaxError: " + error.getMessage(), true);
        }

        if (result.module)
          for (auto error : result.module->errors) {
            auto path = origin(error.location);
            typeDiagnostic(path, error);
          }
      }

      session.instances = roblox ? prepareRoblox(frontend, metadata)
                                  : std::vector<Luau::TypeId>{};

      files.instances = &session.instances;

      auto configurationScope =
          std::make_shared<Luau::Scope>(frontend.globals.globalScope);

      auto configurationDefinition = frontend.loadDefinitionFile(
          frontend.globals, configurationScope, text(configurationTypes),
          "configuration", false);

      if (!configurationDefinition.success)
        throw std::runtime_error("configuration type definition failed");

      auto configurationType = configurationScope->lookupType("Config");

      if (!configurationType)
        throw std::runtime_error("configuration type unavailable");

      auto configurationReturn =
          frontend.globals.globalTypes.addTypePack({configurationType->type});

      frontend.prepareModuleScope = [configurationScope, configurationReturn,
                                     &files](const Luau::ModuleName &name,
                                              const Luau::ScopePtr &scope, bool) {
        if (isConfiguration(name)) {
          for (const auto &binding : configurationScope->exportedTypeBindings)
            scope->exportedTypeBindings[binding.first] = binding.second;

          scope->returnType = configurationReturn;
        } else {
          Bytes index = files.environment(files.context, bytes("script"), bytes(name), 0);

          if (index.data)
            scope->bindings[Luau::AstName("script")] =
                Luau::Binding{files.instances->at(std::stoull(text(index)))};
          else {
            Bytes kind = files.environment(files.context, bytes("kind"), bytes(name), 0);

            if (kind.data)
              if (auto type = scope->lookupType(text(kind)))
                scope->bindings[Luau::AstName("script")] =
                    Luau::Binding{type->type};
          }
        }
      };

      Luau::freeze(frontend.globals.globalTypes);
      initializing = false;
    } else {
      for (const auto &stored : session.diagnostics)
        diagnostic(stored.path, stored.location, stored.message, stored.error);
    }

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

    files.context = nullptr;
    configurations.context = nullptr;
  } catch (const std::exception &error) {
    delete static_cast<Session *>(*handle);
    *handle = nullptr;
    report(context, {}, bytes(error.what()), 0, 0, true);
  } catch (...) {
    delete static_cast<Session *>(*handle);
    *handle = nullptr;
    report(context, {}, bytes("native analysis failure"), 0, 0, true);
  }
}
