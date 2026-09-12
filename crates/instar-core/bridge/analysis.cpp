#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/JsonEmitter.h"
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
#include "color.hpp"
#include <unordered_set>

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

struct Span {
  unsigned line;
  unsigned column;
  unsigned endLine;
  unsigned endColumn;
  bool related;
};

using Read = Bytes (*)(void *, Bytes);
using Resolve = Bytes (*)(void *, Bytes, Bytes, unsigned);
using Environment = Bytes (*)(void *, Bytes, Bytes, size_t);
using Configuration = Bytes (*)(void *, Bytes, size_t);
using Report = void (*)(void *, Bytes, Bytes, Span, bool);
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

  std::string physical(const std::string &name) {
    return text(environment(context, bytes("path"), bytes(name), 0));
  }

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
        report(context, bytes(path), bytes(*error), {}, true);
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
      report(context, {}, bytes(*error), {}, true);

      return;
    }

    for (const auto &entry : configuration.aliases)
      alias(context, bytes(entry.first), bytes(entry.second.value));
  } catch (const std::exception &error) {
    report(context, {}, bytes(error.what()), {}, true);
  } catch (...) {
    report(context, {}, bytes("native configuration failure"), {}, true);
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
  bool related;
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
                               bool oldSolver, bool annotations, const Bytes *changed, size_t changedCount) noexcept {
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

    if (changed && !initializing)
      for (size_t index = 0; index < changedCount; ++index)
        frontend.markDirty(text(changed[index]));
    else
      frontend.clear();

    frontend.clearStats();

    auto diagnostic = [&](const std::string &module,
                          const Luau::Location &location,
                          const std::string &message, bool error) {
      if (initializing)
        session.diagnostics.push_back({module, location, message, error});

      report(context, bytes(module), bytes(message), {location.begin.line,
             location.begin.column, location.end.line, location.end.column}, error);
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

      auto related = [&](const std::string &path, const Luau::Location &location, const std::string &message) {
        if (initializing)
          session.diagnostics.push_back({path, location, message, true, true});

        report(context, bytes(path), bytes(message), {location.begin.line, location.begin.column, location.end.line, location.end.column, true}, true);
      };

      if (auto duplicate = Luau::get_if<Luau::DuplicateTypeDefinition>(&error.data)) {
        if (duplicate->previousLocation)
          related(name, *duplicate->previousLocation, "Previous declaration");
      } else if (auto mismatch = Luau::get_if<Luau::TypeMismatch>(&error.data)) {
        if (mismatch->error && !mismatch->error->moduleName.empty() && mismatch->error->location != Luau::Location{})
          related(mismatch->error->moduleName, mismatch->error->location, Luau::toString(*mismatch->error, Luau::TypeErrorToStringOptions{&files}));
      }
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
        if (stored.related)
          report(context, bytes(stored.path), bytes(stored.message), {stored.location.begin.line, stored.location.begin.column, stored.location.end.line, stored.location.end.column, true}, stored.error);
        else
          diagnostic(stored.path, stored.location, stored.message, stored.error);
    }

    for (size_t index = 0; index < count; ++index)
      if (!loaded.count(text(modules[index])))
        frontend.queueModuleCheck(text(modules[index]));

    for (const std::string &name : frontend.checkQueuedModules()) {
      auto checked = frontend.getCheckResult(name, false);

      if (!checked) {
        report(context, bytes(name),
               bytes("native analysis result unavailable"), {}, true);

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
    report(context, {}, bytes(error.what()), {}, true);
  } catch (...) {
    delete static_cast<Session *>(*handle);
    *handle = nullptr;
    report(context, {}, bytes("native analysis failure"), {}, true);
  }
}

static std::vector<unsigned> coordinates(const Luau::Location &location) {
  return {location.begin.line, location.begin.column, location.end.line, location.end.column};
}

struct EditorAncestry : Luau::AstVisitor {
  Luau::Position position;
  std::vector<Luau::AstNode *> nodes;

  explicit EditorAncestry(Luau::Position position) : position(position) {}

  bool visit(Luau::AstNode *node) override {
    if (!node->location.contains(position))
      return false;

    nodes.push_back(node);

    return true;
  }

  bool visit(Luau::AstType *type) override { return visit(static_cast<Luau::AstNode *>(type)); }
  bool visit(Luau::AstTypePack *) override { return true; }
};

static std::vector<Luau::AstNode *> editorAncestry(const Luau::SourceModule &source, Luau::Position position) {
  EditorAncestry visitor(position);
  source.root->visit(&visitor);

  return visitor.nodes;
}

std::optional<Luau::TypeId> typeAt(const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position) {
  auto ancestry = editorAncestry(source, position);

  if (!ancestry.empty())
    if (auto *annotation = ancestry.back()->asType()) {
      if (auto *type = module.astResolvedTypes.find(annotation))
        return *type;

      return std::nullopt;
    }

  if (!ancestry.empty())
    if (auto *alias = ancestry.back()->as<Luau::AstStatTypeAlias>())
      if (alias->nameLocation.contains(position))
        if (auto scope = Luau::findScopeAtPosition(module, position))
          if (auto type = scope->lookupType(alias->name.value))
            return type->type;

  auto value = Luau::findExprOrLocalAtPosition(source, position);

  if (auto *local = value.getLocal()) {
    if (auto scope = Luau::findScopeAtPosition(module, position))
      return scope->lookup(local);

    return std::nullopt;
  }

  return Luau::findTypeAtPosition(module, source, position);
}

static std::string hoverDescription(const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position, Luau::TypeId type) {
  Luau::ToStringOptions options;
  options.scope = Luau::findScopeAtPosition(module, position);
  options.functionTypeArguments = true;
  auto ancestry = editorAncestry(source, position);
  auto *node = ancestry.empty() ? nullptr : ancestry.back();
  std::string name;
  std::optional<Luau::TypeFun> alias;

  if (node && options.scope) {
    if (auto *declaration = node->as<Luau::AstStatTypeAlias>(); declaration && declaration->nameLocation.contains(position)) {
      name = declaration->name.value;
      alias = options.scope->lookupType(name);
    } else if (auto *reference = node->as<Luau::AstTypeReference>()) {
      name = reference->name.value;

      if (reference->prefix) {
        alias = options.scope->lookupImportedType(reference->prefix->value, name);
        name = std::string(reference->prefix->value) + "." + name;
      } else
        alias = options.scope->lookupType(name);
    }
  }

  if (alias) {
    std::vector<std::string> parameters;

    for (const auto &parameter : alias->typeParams) {
      auto text = Luau::toString(parameter.ty, options);

      if (parameter.defaultValue)
        text += " = " + Luau::toString(*parameter.defaultValue, options);

      parameters.push_back(std::move(text));
    }

    for (const auto &parameter : alias->typePackParams) {
      auto text = Luau::toString(parameter.tp, options);

      if (parameter.defaultValue)
        text += " = " + Luau::toString(*parameter.defaultValue, options);

      parameters.push_back(std::move(text));
    }

    if (!parameters.empty()) {
      name += "<";

      for (size_t index = 0; index < parameters.size(); ++index) {
        if (index)
          name += ", ";

        name += parameters[index];
      }

      name += ">";
    }

    options.exhaustive = true;
    options.useLineBreaks = true;

    return "type " + name + " = " + Luau::toString(alias->type, options);
  }

  auto value = Luau::findExprOrLocalAtPosition(source, position);
  auto *expression = value.getExpr();
  auto *local = value.getLocal();

  if (expression)
    if (auto *reference = expression->as<Luau::AstExprLocal>())
      local = reference->local;

  if (local)
    name = local->name.value;
  else if (expression)
    name = Luau::getFunctionNameAsString(*expression).value_or("");

  if (!node || !node->asType())
    if (auto function = Luau::get<Luau::FunctionType>(Luau::follow(type))) {
      if (expression)
        if (auto *index = expression->as<Luau::AstExprIndexName>(); index && index->op == ':') {
          options.hideFunctionSelfArgument = true;
          auto separator = name.rfind('.');

          if (separator != std::string::npos)
            name[separator] = ':';
        }

      return "function " + Luau::toStringNamedFunction(name, *function, options);
    }

  options.exhaustive = true;
  options.useLineBreaks = true;
  auto description = Luau::toString(type, options);

  if (node && node->asType())
    return description;

  if (local)
    return std::string(local->isConst ? "const " : "local ") + name + ": " + description;

  return name.empty() ? description : name + ": " + description;
}

struct Destination {
  std::string path;
  Luau::Location location;
  std::string name;
};

static std::optional<Destination> destination(Luau::Frontend &frontend, Luau::Module &module, const Luau::SourceModule &source,
                                              const std::string &path, Luau::Position position, bool typeOnly) {
  auto ancestry = editorAncestry(source, position);

  if (ancestry.empty())
    return std::nullopt;

  auto *node = ancestry.back();

  if (!typeOnly) {
    for (auto *ancestor : ancestry)
      if (auto *table = ancestor->as<Luau::AstExprTable>())
        for (const auto &item : table->items)
          if (item.kind == Luau::AstExprTable::Item::Kind::Record && item.key->location.contains(position))
            return Destination{path, item.key->location};

    auto value = Luau::findExprOrLocalAtPosition(source, position);

    if (auto *local = value.getLocal())
      return Destination{path, local->location};

    if (auto *local = node->as<Luau::AstExprLocal>())
      return Destination{path, local->local->location};

    if (auto *alias = node->as<Luau::AstStatTypeAlias>())
      if (alias->nameLocation.contains(position))
        return Destination{path, alias->nameLocation};

    if (auto *index = node->as<Luau::AstExprIndexName>())
      if (auto owner = module.astTypes.find(index->expr))
        if (auto table = Luau::get<Luau::TableType>(Luau::follow(*owner))) {
          auto property = table->props.find(index->index.value);

          if (property != table->props.end() && property->second.location)
            return Destination{table->definitionModuleName.empty() ? path : table->definitionModuleName, *property->second.location};
        }
  }

  if (auto *reference = node->as<Luau::AstTypeReference>()) {
    for (auto scope = Luau::findScopeAtPosition(module, position); scope; scope = scope->parent) {
      if (!typeOnly && reference->prefix && reference->prefixLocation && reference->prefixLocation->contains(position))
        if (auto binding = scope->linearSearchForBinding(reference->prefix->value))
          return Destination{path, binding->location};

      if (!reference->prefix) {
        auto declaration = scope->typeAliasNameLocations.find(reference->name.value);

        if (declaration != scope->typeAliasNameLocations.end())
          return Destination{path, declaration->second};
      } else {
        auto imported = scope->importedModules.find(reference->prefix->value);

        if (imported != scope->importedModules.end())
          if (auto *parsed = frontend.getSourceModule(imported->second))
            for (auto *statement : parsed->root->body)
              if (auto *alias = statement->as<Luau::AstStatTypeAlias>(); alias && alias->exported && std::string_view(alias->name.value) == reference->name.value)
                return Destination{imported->second, alias->nameLocation};
      }
    }

    return std::nullopt;
  }

  if (auto type = typeAt(module, source, position)) {
    auto followed = Luau::follow(*type);

    if (auto function = Luau::get<Luau::FunctionType>(followed))
      if (function->definition) {
        const auto &definition = *function->definition;
        Destination target{definition.definitionModuleName.value_or(path), definition.originalNameLocation};

        if (target.location.begin == target.location.end)
          target.location = definition.definitionLocation;
        else if (target.location.end.column)
          if (auto *parsed = frontend.getSourceModule(target.path))
            if (auto *expression = Luau::findExprAtPosition(*parsed, {target.location.end.line, target.location.end.column - 1}))
              if (auto *index = expression->as<Luau::AstExprIndexName>())
                target.location = index->indexLocation;

        return target;
      }

    if (typeOnly) {
      if (auto table = Luau::get<Luau::TableType>(followed))
        return Destination{table->definitionModuleName.empty() ? path : table->definitionModuleName, table->definitionLocation};

      if (auto external = Luau::get<Luau::ExternType>(followed))
        if (external->definitionLocation)
          return Destination{external->definitionModuleName, *external->definitionLocation};
    }
  }

  return std::nullopt;
}

static std::string documentation(Luau::Frontend &frontend, Files &files, const Destination &target) {
  auto *source = frontend.getSourceModule(target.path);

  if (!source)
    return {};

  auto contents = text(files.read(files.context, bytes(target.path)));
  std::vector<size_t> lines{0};

  for (size_t index = 0; index < contents.size(); ++index)
    if (contents[index] == '\n')
      lines.push_back(index + 1);

  if (target.location.begin.line >= lines.size())
    return {};

  size_t boundary = lines[target.location.begin.line];
  std::vector<std::string> comments;

  for (auto iterator = source->commentLocations.rbegin(); iterator != source->commentLocations.rend(); ++iterator) {
    size_t begin = lines.at(iterator->location.begin.line) + iterator->location.begin.column;
    size_t end = lines.at(iterator->location.end.line) + iterator->location.end.column;

    if (end > boundary)
      continue;

    if (contents.substr(end, boundary - end).find_first_not_of(" \t\r\n") != std::string::npos ||
        contents.substr(lines.at(iterator->location.begin.line), iterator->location.begin.column).find_first_not_of(" \t") != std::string::npos)
      break;

    auto comment = contents.substr(begin, end - begin);

    if (comment == "---")
      comments.emplace_back();
    else if (comment.compare(0, 4, "--- ") == 0)
      comments.push_back(comment.substr(4));
    else if (comment.compare(0, 3, "--[") == 0) {
      size_t opening = comment.find_first_not_of('=', 3);

      if (opening == std::string::npos || comment[opening] != '[')
        break;

      size_t ending = opening - 1;

      if (comment.size() < opening + 1 + ending)
        break;

      comments.push_back(comment.substr(opening + 1, comment.size() - opening - 1 - ending));
    } else
      break;

    boundary = begin;
  }

  std::string result;

  for (auto iterator = comments.rbegin(); iterator != comments.rend(); ++iterator) {
    if (!result.empty())
      result += "\n";

    result += *iterator;
  }

  return result;
}

struct EditorSymbols : Luau::AstVisitor {
  Files &files;
  Luau::Frontend &frontend;
  Luau::Json::JsonEmitter &output;
  Luau::Module &module;
  const Luau::SourceModule &source;
  std::string path;
  std::optional<Destination> target;
  bool tokens;
  bool links;
  std::unordered_set<Luau::AstLocal *> parameters;

  EditorSymbols(Files &files, Luau::Frontend &frontend, Luau::Json::JsonEmitter &output, Luau::Module &module, const Luau::SourceModule &source,
                std::string path, std::optional<Destination> target, bool tokens, bool links)
      : files(files), frontend(frontend), output(output), module(module), source(source), path(std::move(path)), target(std::move(target)), tokens(tokens), links(links) {
    if (this->target)
      this->target->path = files.physical(this->target->path);
  }

  void entry(const std::string &name, Luau::Location selection, Luau::Location range, unsigned kind, bool declaration, unsigned modifiers = 0) {
    if (target) {
      if (!target->name.empty() && name != target->name)
        return;

      auto resolved = destination(frontend, module, source, path, selection.begin, false);

      if (!(declaration && files.physical(path) == target->path && selection == target->location) &&
          (!resolved || files.physical(resolved->path) != target->path || resolved->location != target->location))
        return;
    } else if (!tokens && !links && !declaration)
      return;

    writeEntry(name, selection, range, kind, declaration, modifiers);
  }

  void writeEntry(const std::string &name, Luau::Location selection, Luau::Location range, unsigned kind, bool declaration, unsigned modifiers = 0) {
    if (links && kind != 3)
      return;

    output.writeComma();
    auto object = output.writeObject();
    object.writePair("name", name);
    object.writePair("path", path);
    object.writePair("range", coordinates(tokens || target ? selection : range));
    object.writePair("selection", coordinates(selection));
    object.writePair("kind", kind);
    object.writePair("declaration", declaration);
    object.writePair("modifiers", modifiers);
  }

  void local(Luau::AstLocal *local, Luau::Location range) {
    if (links)
      return;

    if (target && (files.physical(path) != target->path || local->location != target->location))
      return;

    unsigned kind = 13;

    if (auto scope = Luau::findScopeAtPosition(module, local->location.begin))
      if (auto type = scope->lookup(local))
        if (Luau::get<Luau::FunctionType>(Luau::follow(*type)))
          kind = 12;

    writeEntry(local->name.value, local->location, range, tokens && parameters.count(local) ? 27 : kind, true, local->isConst ? 2 : 0);
  }

  bool visit(Luau::AstStatLocal *statement) override {
    for (auto *value : statement->vars)
      local(value, statement->location);

    return true;
  }

  bool visit(Luau::AstStatLocalFunction *statement) override {
    local(statement->name, statement->location);

    return true;
  }

  bool visit(Luau::AstStatFunction *statement) override {
    if (auto *global = statement->name->as<Luau::AstExprGlobal>())
      entry(global->name.value, global->location, statement->location, 12, true);
    else if (auto *index = statement->name->as<Luau::AstExprIndexName>()) {
      entry(index->index.value, index->indexLocation, statement->location, tokens && index->op == ':' ? 6 : 12, true);
      index->expr->visit(this);
    } else
      statement->name->visit(this);

    statement->func->visit(this);

    return false;
  }

  bool visit(Luau::AstExprFunction *function) override {
    if (function->self) parameters.insert(function->self);

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
    for (auto *value : statement->vars)
      local(value, value->location);

    return true;
  }

  bool visit(Luau::AstExprTable *expression) override {
    for (const auto &item : expression->items)
      if (item.kind == Luau::AstExprTable::Item::Kind::Record)
        if (auto *key = item.key->as<Luau::AstExprConstantString>())
          entry(std::string(key->value.data, key->value.size), key->location, Luau::Location(key->location.begin, item.value->location.end), item.value->is<Luau::AstExprFunction>() ? 12 : 7, true);

    return true;
  }

  bool visit(Luau::AstType *type) override { return true; }
  bool visit(Luau::AstTypePack *) override { return true; }

  bool visit(Luau::AstExprCall *call) override {
    if ((tokens || links) && !target && call->args.size == 1)
      if (auto *global = call->func->as<Luau::AstExprGlobal>(); global && global->name == "require") {
        auto *argument = call->args.data[0];

        if (links || argument->is<Luau::AstExprConstantString>()) {
          auto trace = frontend.requireTrace.find(path);

          if (trace != frontend.requireTrace.end())
            if (auto resolved = trace->second.exprs.find(argument))
              if (frontend.getSourceModule(resolved->name))
                entry(files.physical(resolved->name), argument->location, argument->location, 3, false);
        }
      }

    return true;
  }

  bool visit(Luau::AstGenericType *generic) override {
    if (tokens) entry(generic->name.value, generic->location, generic->location, 28, true);

    return true;
  }

  bool visit(Luau::AstGenericTypePack *generic) override {
    if (tokens) entry(generic->name.value, generic->location, generic->location, 28, true);

    return true;
  }

  bool visit(Luau::AstTypeReference *reference) override {
    if (reference->prefix && reference->prefixLocation)
      entry(reference->prefix->value, *reference->prefixLocation, *reference->prefixLocation, 13, false);

    unsigned kind = 26;

    if (tokens)
      if (auto type = module.astResolvedTypes.find(reference)) {
        auto followed = Luau::follow(*type);

        if (Luau::get<Luau::GenericType>(followed)) kind = 28;
        else if (Luau::get<Luau::ExternType>(followed)) kind = 5;
      }

    entry(reference->name.value, reference->nameLocation, reference->nameLocation, kind, false);

    return true;
  }

  bool visit(Luau::AstStatTypeAlias *statement) override {
    entry(statement->name.value, statement->nameLocation, statement->location, 26, true);

    return true;
  }

  bool visit(Luau::AstExprLocal *expression) override {
    if (target) {
      if (files.physical(path) == target->path && expression->local->location == target->location)
        writeEntry(expression->local->name.value, expression->location, expression->location, 13, false);
    } else {
      unsigned kind = 13;

      if (tokens) {
        if (parameters.count(expression->local)) kind = 27;
        else if (auto type = module.astTypes.find(expression))
          if (Luau::get<Luau::FunctionType>(Luau::follow(*type))) kind = 12;
      }

      entry(expression->local->name.value, expression->location, expression->location, kind, false, expression->local->isConst ? 2 : 0);
    }

    return true;
  }

  bool visit(Luau::AstExprGlobal *expression) override {
    entry(expression->name.value, expression->location, expression->location, 13, false);

    return true;
  }

  bool visit(Luau::AstExprIndexName *expression) override {
    unsigned kind = 7;

    if (tokens)
      if (auto type = module.astTypes.find(expression))
        if (Luau::get<Luau::FunctionType>(Luau::follow(*type))) kind = expression->op == ':' ? 6 : 12;

    entry(expression->index.value, expression->indexLocation, expression->indexLocation, kind, false);

    return true;
  }
};

struct EditorCalls : Luau::AstVisitor {
  Luau::Frontend &frontend;
  Luau::Module &module;
  const Luau::SourceModule &source;
  Luau::Json::JsonEmitter &output;
  std::string path;

  EditorCalls(Luau::Frontend &frontend, Luau::Module &module, const Luau::SourceModule &source, Luau::Json::JsonEmitter &output, std::string path)
      : frontend(frontend), module(module), source(source), output(output), path(std::move(path)) {}

  bool visit(Luau::AstExprCall *call) override {
    auto position = call->func->location.end;

    if (!position.column) return true;

    --position.column;
    auto target = destination(frontend, module, source, path, position, false);

    if (!target) return true;

    output.writeComma();
    auto object = output.writeObject();
    object.writePair("path", target->path);
    object.writePair("range", coordinates(target->location));
    object.writePair("caller", coordinates(call->func->location));
    auto ancestry = editorAncestry(source, call->location.begin);

    for (auto iterator = ancestry.rbegin(); iterator != ancestry.rend(); ++iterator)
      if (auto *function = (*iterator)->as<Luau::AstExprFunction>()) {
        object.writePair("container", coordinates(function->location));
        break;
      }

    return true;
  }
};

static void rangeEntry(Luau::Json::JsonEmitter &output, Luau::Location location) {
  output.writeComma();
  auto object = output.writeObject();
  object.writePair("range", coordinates(location));
}

struct FoldingRanges : Luau::AstVisitor {
  Luau::Json::JsonEmitter &output;
  Luau::AstNode *root;
  FoldingRanges(Luau::Json::JsonEmitter &output, Luau::AstNode *root) : output(output), root(root) {}

  bool visit(Luau::AstNode *node) override {
    if (node != root && node->location.begin.line < node->location.end.line)
      rangeEntry(output, node->location);

    return true;
  }

  bool visit(Luau::AstType *type) override { return visit(static_cast<Luau::AstNode *>(type)); }
  bool visit(Luau::AstTypePack *) override { return true; }
};

extern "C" void instar_query(void *handle, void *context, Bytes name, unsigned line, unsigned column,
                              Bytes operation, Annotate output) noexcept {
  std::lock_guard<std::mutex> lock(analysisMutex);
  auto *session = static_cast<Session *>(handle);

  if (!session) {
    output(context, {}, bytes("null"));

    return;
  }

  session->files.context = context;
  session->configurations.context = context;

  try {
    auto &frontend = *session->frontend;
    std::string path = text(name);
    std::string command = text(operation);
    Luau::Position position{line, column};
    auto module = frontend.moduleResolver.getModule(path);
    auto *source = frontend.getSourceModule(path);
    Luau::Json::JsonEmitter result;

    if (!module || !source)
      Luau::Json::write(result, nullptr);
    else if (command == "calls") {
      auto array = result.writeArray();
      EditorCalls visitor(frontend, *module, *source, result, path);
      source->root->visit(&visitor);
    } else if (command == "colors") {
      auto array = result.writeArray();
      Colors visitor(result);
      source->root->visit(&visitor);
    } else if (command == "folds") {
      auto array = result.writeArray();
      FoldingRanges visitor(result, source->root);
      source->root->visit(&visitor);

      for (const auto &comment : source->commentLocations)
        if (comment.location.begin.line < comment.location.end.line)
          rangeEntry(result, comment.location);
    } else if (command == "selection") {
      auto array = result.writeArray();
      auto ancestry = editorAncestry(*source, position);

      for (auto *node : ancestry)
        rangeEntry(result, node->location);

      if (!ancestry.empty()) {
        if (auto *index = ancestry.back()->as<Luau::AstExprIndexName>(); index && index->indexLocation.contains(position))
          rangeEntry(result, index->indexLocation);
        else if (auto *alias = ancestry.back()->as<Luau::AstStatTypeAlias>(); alias && alias->nameLocation.contains(position))
          rangeEntry(result, alias->nameLocation);
      }
    } else if (command == "scope") {
      auto value = Luau::findExprOrLocalAtPosition(*source, position);
      bool local = value.getLocal() || (value.getExpr() && value.getExpr()->is<Luau::AstExprLocal>());
      auto object = result.writeObject();

      if (local)
        object.writePair("kind", 13);
    } else if (command == "completion") {
      auto completions = Luau::autocomplete(frontend, path, position, {});
      auto array = result.writeArray();

      if (completions.context == Luau::AutocompleteContext::Expression || completions.context == Luau::AutocompleteContext::Statement) {
        result.writeComma();
        auto object = result.writeObject();
        object.writePair("imports", true);
        unsigned line = 0;

        for (const auto &comment : source->hotcomments)
          if (comment.header) line = comment.location.end.line + 1;

        object.writePair("range", std::vector<unsigned>{line, 0, line, 0});
      }

      for (const auto &[label, completion] : completions.entryMap) {
        result.writeComma();
        auto object = result.writeObject();
        object.writePair("name", label);
        object.writePair("type", completion.type ? Luau::toString(*completion.type) : std::string{});
        object.writePair("documentation", completion.documentationSymbol.value_or(""));
        object.writePair("insert", completion.insertText.value_or(label));
        object.writePair("deprecated", completion.deprecated);

        if (completion.type)
          if (auto function = Luau::get<Luau::FunctionType>(Luau::follow(*completion.type)); function && function->definition)
            object.writePair("documentation_text", documentation(frontend, session->files, {function->definition->definitionModuleName.value_or(path), function->definition->definitionLocation}));
      }
    } else if (command == "symbols" || command == "tokens" || command == "links" || command == "references" || command == "localReferences" || command == "prepare") {
      auto target = command == "references" || command == "localReferences" || command == "prepare" ? destination(frontend, *module, *source, path, position, false) : std::nullopt;
      auto array = result.writeArray();

      if (target && target->location.begin.line == target->location.end.line) {
        auto contents = text(session->files.read(context, bytes(target->path)));
        size_t offset = 0;

        for (unsigned line = 0; line < target->location.begin.line; ++line) {
          auto newline = contents.find('\n', offset);

          if (newline == std::string::npos)
            break;

          offset = newline + 1;
        }

        offset += target->location.begin.column;

        if (offset <= contents.size())
          target->name = contents.substr(offset, target->location.end.column - target->location.begin.column);
      }

      if ((command != "references" && command != "localReferences" && command != "prepare") || target) {
        for (const auto &[moduleName, node] : frontend.sourceNodes) {
          if (command != "references" && moduleName != path &&
              !(command == "prepare" && target && session->files.physical(moduleName) == session->files.physical(target->path)))
            continue;

          auto checked = frontend.moduleResolver.getModule(moduleName);
          auto *parsed = frontend.getSourceModule(moduleName);

          if (checked && parsed) {
            EditorSymbols visitor(session->files, frontend, result, *checked, *parsed, moduleName, target, command == "tokens", command == "links");
            parsed->root->visit(&visitor);
          }
        }
      }
    } else if (command == "definition" || command == "typeDefinition") {
      auto target = destination(frontend, *module, *source, path, position, command == "typeDefinition");

      if (!target && command == "definition") {
        auto ancestry = Luau::findAstAncestryOfPosition(*source, position, true);

        for (auto *node : ancestry)
          if (auto *call = node->as<Luau::AstExprCall>())
            if (auto *global = call->func->as<Luau::AstExprGlobal>(); global && global->name == "require" && call->args.size) {
              Luau::ModuleInfo from{path};

              if (auto resolved = session->files.resolveModule(&from, call->args.data[0], {}))
                target = Destination{resolved->name, {}};
            }
      }

      if (target) {
        auto object = result.writeObject();
        object.writePair("path", target->path);
        object.writePair("range", coordinates(target->location));
      } else
        Luau::Json::write(result, nullptr);
    } else if (command == "signature") {
      auto ancestry = Luau::findAstAncestryOfPosition(*source, position, true);
      auto array = result.writeArray();

      for (auto iterator = ancestry.rbegin(); iterator != ancestry.rend(); ++iterator) {
        auto *call = (*iterator)->as<Luau::AstExprCall>();

        if (!call || position < call->func->location.end)
          continue;

        if (auto type = module->astTypes.find(call->func)) {
          std::vector<Luau::TypeId> signatures{Luau::follow(*type)};

          if (auto intersection = Luau::get<Luau::IntersectionType>(signatures.front()))
            signatures = intersection->parts;

          for (auto signature : signatures) {
            auto function = Luau::get<Luau::FunctionType>(Luau::follow(signature));

            if (!function)
              continue;

            Luau::ToStringOptions options;
            options.functionTypeArguments = true;
            options.hideFunctionSelfArgument = call->self;
            auto label = Luau::toString(signature, options);
            auto [arguments, tail] = Luau::flatten(function->argTypes);
            std::vector<std::string> parameters;

            for (size_t index = call->self && function->hasSelf ? 1 : 0; index < arguments.size(); ++index) {
              auto parameter = Luau::toString(arguments[index], options);

              if (index < function->argNames.size() && function->argNames[index])
                parameter = function->argNames[index]->name + ": " + parameter;

              parameters.push_back(std::move(parameter));
            }

            if (tail)
              parameters.push_back(Luau::toString(*tail, options));

            result.writeComma();
            auto object = result.writeObject();
            object.writePair("label", label);
            object.writePair("parameters", parameters);
            object.writePair("documentation", Luau::getDocumentationSymbolAtPosition(*source, *module, call->func->location.begin).value_or(""));

            if (function->definition)
              object.writePair("documentation_text", documentation(frontend, session->files, {function->definition->definitionModuleName.value_or(path), function->definition->definitionLocation}));

            unsigned active = 0;

            for (auto *argument : call->args)
              if (argument->location.end < position)
                ++active;

            if (!parameters.empty())
              object.writePair("active", std::min(active, unsigned(parameters.size() - 1)));
          }

          break;
        }
      }
    } else {
      auto value = Luau::findExprOrLocalAtPosition(*source, position);
      auto type = typeAt(*module, *source, position);

      if (type) {
        auto location = value.getLocation().value_or(Luau::Location{position, position});
        auto ancestry = editorAncestry(*source, position);

        if (!ancestry.empty()) {
          auto *node = ancestry.back();

          if (auto *annotation = node->asType())
            location = annotation->location;
          else if (auto *alias = node->as<Luau::AstStatTypeAlias>(); alias && alias->nameLocation.contains(position))
            location = alias->nameLocation;
          else if (auto *index = node->as<Luau::AstExprIndexName>())
            location = index->indexLocation;
        }

        auto object = result.writeObject();
        object.writePair("type", hoverDescription(*module, *source, position, *type));
        object.writePair("name", value.getName() ? value.getName()->value : "");
        object.writePair("range", coordinates(location));
        auto symbol = Luau::follow(*type)->documentationSymbol;

        if (!symbol && (ancestry.empty() || !ancestry.back()->asType()))
          symbol = Luau::getDocumentationSymbolAtPosition(*source, *module, position);

        object.writePair("documentation", symbol.value_or(""));

        if (auto target = destination(frontend, *module, *source, path, position, false))
          object.writePair("documentation_text", documentation(frontend, session->files, *target));
      } else
        Luau::Json::write(result, nullptr);
    }

    output(context, {}, bytes(result.str()));
  } catch (const std::exception &error) {
    Luau::Json::JsonEmitter result;

    { auto object = result.writeObject(); object.writePair("error", error.what()); }

    output(context, {}, bytes(result.str()));
  } catch (...) {
    output(context, {}, bytes("{\"error\":\"native editor query failed\"}"));
  }

  session->files.context = nullptr;
  session->configurations.context = nullptr;
}
