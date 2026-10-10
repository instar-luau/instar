#include "analysis.hpp"
#include "parsing.hpp"

#include "Luau/Ast.h"

#include <map>
#include <optional>
#include <set>
#include <stdexcept>
#include <string>
#include <utility>

namespace instar::native {
    namespace {
        struct Calls final : Luau::AstVisitor {
            const NativeOperation &host;
            const std::string &name;
            std::map<std::pair<size_t, size_t>, Luau::AstExprCall *> calls;
            std::map<const Luau::AstLocal *, const Luau::AstExpr *> initializers;
            std::set<const Luau::AstLocal *> writes;
            std::set<std::string> global_writes;

            Calls(const NativeOperation &host, const std::string &name) : host(host), name(name) {}

            bool visit(Luau::AstExprCall *call) override {
                calls.emplace(
                    std::make_pair(offset(host, name, call->location.begin), offset(host, name, call->location.end)),
                    call
                );

                return true;
            }

            bool visit(Luau::AstStatLocal *statement) override {
                for (size_t index = 0; index < statement->vars.size && index < statement->values.size; ++index) {
                    initializers.emplace(statement->vars.data[index], statement->values.data[index]);
                }

                return true;
            }

            bool visit(Luau::AstStatAssign *statement) override {
                for (const auto *variable : statement->vars) {
                    assigned(variable);
                }

                return true;
            }

            bool visit(Luau::AstStatCompoundAssign *statement) override {
                assigned(statement->var);

                return true;
            }

            bool visit(Luau::AstStatFunction *statement) override {
                assigned(statement->name);

                return true;
            }

            bool visit(Luau::AstStatTypeFunction *) override { return false; }

            void assigned(const Luau::AstExpr *expression) {
                if (const auto *local = expression->as<Luau::AstExprLocal>()) {
                    writes.insert(local->local);
                } else if (const auto *global = expression->as<Luau::AstExprGlobal>()) {
                    global_writes.insert(global->name.value);
                }
            }

            bool visit(Luau::AstType *) override { return true; }

            bool visit(Luau::AstTypePack *) override { return true; }
        };

        const Luau::AstExpr *initializer(const Luau::AstExpr *expression, const Calls &calls, size_t depth = 0) {
            if (depth > 128) {
                return nullptr;
            }

            if (const auto *group = expression->as<Luau::AstExprGroup>()) {
                return initializer(group->expr, calls, depth + 1);
            }

            if (const auto *assertion = expression->as<Luau::AstExprTypeAssertion>()) {
                return initializer(assertion->expr, calls, depth + 1);
            }

            if (const auto *instantiation = expression->as<Luau::AstExprInstantiate>()) {
                return initializer(instantiation->expr, calls, depth + 1);
            }

            if (const auto *local = expression->as<Luau::AstExprLocal>()) {
                const auto found = calls.initializers.find(local->local);

                if (found == calls.initializers.end() || calls.writes.count(local->local)) {
                    return nullptr;
                }

                return initializer(found->second, calls, depth + 1);
            }

            return expression;
        }

        bool loader(const Luau::AstExpr *expression, const Calls &calls) {
            const auto *value = initializer(expression, calls);
            const auto *global = value ? value->as<Luau::AstExprGlobal>() : nullptr;

            return global && std::string(global->name.value) == "require";
        }

        std::optional<std::string> string_value(const Luau::AstExpr *expression, const Calls &calls, size_t depth = 0) {
            if (depth > 128) {
                return std::nullopt;
            }

            expression = initializer(expression, calls);

            if (!expression) {
                return std::nullopt;
            }

            if (const auto *string = expression->as<Luau::AstExprConstantString>()) {
                const auto document = calls.host.read_document(calls.name);
                const auto start = offset(calls.host, calls.name, string->location.begin);

                if (start >= document.text.size() ||
                    (document.text.data()[start] != '"' && document.text.data()[start] != '\'' &&
                        document.text.data()[start] != '[')) {
                    return std::nullopt;
                }

                try {
                    return std::string(rust::String(string->value.data, string->value.size));
                } catch (const std::invalid_argument &) {
                    return std::nullopt;
                }
            }

            if (const auto *binary = expression->as<Luau::AstExprBinary>();
                binary && binary->op == Luau::AstExprBinary::Concat) {
                const auto left = string_value(binary->left, calls, depth + 1);
                const auto right = string_value(binary->right, calls, depth + 1);

                if (left && right) {
                    return *left + *right;
                }
            }

            return std::nullopt;
        }

        std::string component(const std::string &kind, const std::string &name) {
            return "/" + kind + ":" + std::to_string(name.size()) + ":" + name;
        }

        std::optional<std::string>
        instance_path(const Luau::AstExpr *expression, const Calls &calls, size_t depth = 0) {
            if (depth > 128) {
                return std::nullopt;
            }

            expression = initializer(expression, calls);

            if (!expression) {
                return std::nullopt;
            }

            if (const auto *global = expression->as<Luau::AstExprGlobal>()) {
                const std::string root(global->name.value);

                if (!calls.global_writes.count(root) && (root == "script" || root == "game" || root == "workspace")) {
                    return root;
                }
            }

            if (const auto *index = expression->as<Luau::AstExprIndexName>()) {
                const auto parent = instance_path(index->expr, calls, depth + 1);
                const std::string name(index->index.value);

                return parent ? std::optional<std::string>{*parent +
                                                           (name == "Parent" ? "/parent" : component("field", name))}
                              : std::nullopt;
            }

            if (const auto *index = expression->as<Luau::AstExprIndexExpr>()) {
                const auto parent = instance_path(index->expr, calls, depth + 1);
                const auto argument = string_value(index->index, calls);

                if (parent && argument) {
                    const std::string &name = *argument;

                    return *parent + (name == "Parent" ? "/parent" : component("field", name));
                }
            }

            if (const auto *call = expression->as<Luau::AstExprCall>();
                call && call->self && call->args.size >= 1 && call->args.size <= 2) {
                const auto *method = call->func->as<Luau::AstExprIndexName>();
                const auto argument = string_value(call->args.data[0], calls);

                if (!method || !argument) {
                    return std::nullopt;
                }

                const auto parent = instance_path(method->expr, calls, depth + 1);
                const std::string &name = *argument;
                const std::string operation(method->index.value);

                if (!parent) {
                    return std::nullopt;
                }

                if (operation == "GetService" && call->args.size == 1) {
                    return *parent + component("service", name);
                }

                if (operation == "WaitForChild") {
                    return *parent + component("child:0", name);
                }

                if (operation == "FindFirstChild") {
                    bool recursive = false;

                    if (call->args.size == 2) {
                        const auto *flag = initializer(call->args.data[1], calls);
                        const auto *boolean = flag ? flag->as<Luau::AstExprConstantBool>() : nullptr;

                        if (!boolean) {
                            return std::nullopt;
                        }

                        recursive = boolean->value;
                    }

                    return *parent + component(recursive ? "child:1" : "child:0", name);
                }
            }

            return std::nullopt;
        }
    }

    void parse_modules(AnalysisState &state, const NativeOperation &host, rust::Slice<const rust::String> names) {
        auto &frontend = state.frontend;
        state.resolver.host = &host;

        struct Reset {
            Resolver &resolver;

            ~Reset() { resolver.host = nullptr; }
        } reset{state.resolver};

        std::vector<Luau::ModuleName> modules;

        for (const rust::String &name : names) {
            modules.emplace_back(name);
        }

        host.progress("Preparing native graph", "", 0, modules.size());
        frontend.parseModules(modules);

        size_t completed = 0;

        for (const std::string &name : modules) {
            const NativeModule source = host.read_module(name);
            Luau::SourceModule *module = frontend.getSourceModule(name);
            const auto node = frontend.sourceNodes.find(name);

            if (!source.found || !module || !module->root || node == frontend.sourceNodes.end()) {
                throw std::runtime_error("native module source is unavailable: " + name);
            }

            Calls calls(host, name);

            module->root->visit(&calls);

            for (const auto &[range, call] : calls.calls) {
                if (loader(call->func, calls) && !host.require(name, range.first, range.second)) {
                    throw std::invalid_argument("captured import site is missing: " + name);
                }
            }

            Luau::RequireTraceResult trace;
            node->second->requireSet.clear();
            node->second->requireLocations.clear();

            for (const NativeSite &site : source.sites) {
                const auto found = calls.calls.find({site.call_start, site.call_end});

                if (found == calls.calls.end()) {
                    throw std::invalid_argument("host/native static require call disagreement: " + name);
                }

                Luau::AstExprCall *call = found->second;

                if (!loader(call->func, calls)) {
                    throw std::invalid_argument("captured import call is not the Luau loader: " + name);
                }

                const auto argument_start = call->args.size ? offset(host, name, call->args.data[0]->location.begin)
                                                            : offset(host, name, call->argLocation.begin) - 1;

                const auto argument_end =
                    offset(host, name, call->args.size ? call->args.data[0]->location.end : call->argLocation.end);

                if (argument_start != site.argument_start || argument_end != site.argument_end) {
                    throw std::invalid_argument("host/native require argument disagreement: " + name);
                }

                const bool stable = !calls.global_writes.count("require") && call->args.size == 1;
                const auto string = stable ? string_value(call->args.data[0], calls) : std::nullopt;
                const auto path = stable && !string ? instance_path(call->args.data[0], calls) : std::nullopt;
                const auto request_kind = static_cast<uint8_t>(string ? 0 : path ? 1 : 2);

                if (site.request_kind != request_kind || site.static_request != (request_kind != 2) ||
                    std::string(site.request_text) != (string    ? *string
                                                          : path ? *path
                                                                 : "")) {
                    throw std::invalid_argument("captured import request disagrees with native source: " + name);
                }

                if (!site.static_request && !site.target.empty()) {
                    throw std::invalid_argument("nonconstant import has a captured target");
                }

                const std::string target(site.target);

                if (target.empty()) {
                    if (site.static_request) {
                        trace.exprs[call->args.data[0]] = Luau::ModuleInfo{};
                        trace.exprs[call] = Luau::ModuleInfo{};
                    }

                    continue;
                }

                if (frontend.sourceNodes.find(target) == frontend.sourceNodes.end()) {
                    throw std::runtime_error("host target was not parsed: " + target);
                }

                trace.exprs[call->args.data[0]] = Luau::ModuleInfo{target};
                trace.exprs[call] = Luau::ModuleInfo{target};
                trace.requireList.emplace_back(target, call->location);
                node->second->requireSet.insert(target);
                node->second->requireLocations.emplace_back(target, call->location);
            }

            frontend.requireTrace.insert_or_assign(name, std::move(trace));
            host.progress("Preparing native graph", name, ++completed, modules.size());
        }

        for (const auto &entry : frontend.sourceNodes) {
            entry.second->dependents.clear();
        }

        for (const auto &[name, node] : frontend.sourceNodes) {
            for (const std::string &target : node->requireSet) {
                if (const auto found = frontend.sourceNodes.find(target); found != frontend.sourceNodes.end()) {
                    found->second->dependents.insert(name);
                }
            }
        }
    }
}

namespace instar::native {
    size_t offset(const NativeOperation &host, const std::string &name, Luau::Position position) {
        return host.offset(name, position.line, position.column);
    }

    NativeKind kind(const Luau::TypeError &error) {
        if (Luau::get<Luau::SyntaxError>(error)) {
            return NativeKind::Syntax;
        }

        if (Luau::get<Luau::UnknownRequire>(error) || Luau::get<Luau::IllegalRequire>(error)) {
            return NativeKind::Resolution;
        }

        if (Luau::get<Luau::CodeTooComplex>(error) || Luau::get<Luau::UnificationTooComplex>(error) ||
            Luau::get<Luau::NormalizationTooComplex>(error) || Luau::get<Luau::ModuleHasCyclicDependency>(error) ||
            Luau::get<Luau::ConstraintSolvingIncompleteError>(error) || Luau::get<Luau::InternalError>(error)) {
            return NativeKind::Analysis;
        }

        return NativeKind::Type;
    }

    NativeLocation location(const NativeOperation &host, const Luau::ModuleName &name, Luau::Location range) {
        return NativeLocation{rust::String(name),
            host.revision(name),
            offset(host, name, range.begin),
            offset(host, name, range.end)};
    }

    NativeDiagnostic
    diagnostic(const NativeOperation &host, const Luau::TypeError &error, const std::string &fallback) {
        const std::string name = error.moduleName.empty() ? fallback : error.moduleName;

        NativeDiagnostic result{location(host, name, error.location),
            kind(error),
            error.code(),
            rust::String(Luau::toString(error)),
            {}};

        const auto *count = Luau::get<Luau::CountMismatch>(error);

        if (Luau::get<Luau::GenericError>(error) ||
            (count && count->context == Luau::CountMismatch::Arg && count->expected == 1 && count->actual != 1)) {
            if (host.require(name, result.location.start, result.location.end)) {
                result.kind = NativeKind::Resolution;
            }
        }

        if (const auto *duplicate = Luau::get<Luau::DuplicateTypeDefinition>(error);
            duplicate && duplicate->previousLocation) {
            result.related.push_back(
                NativeRelated{location(host, name, *duplicate->previousLocation),
                    rust::String("Previous type definition")}
            );
        }

        const Luau::TypeError *nested = &error;

        while (const auto *mismatch = Luau::get<Luau::TypeMismatch>(*nested)) {
            if (!mismatch->error) {
                break;
            }

            nested = mismatch->error.get();
            const std::string related_name = nested->moduleName.empty() ? name : nested->moduleName;

            result.related.push_back(
                NativeRelated{location(host, related_name, nested->location), rust::String(Luau::toString(*nested))}
            );
        }

        return result;
    }
}
