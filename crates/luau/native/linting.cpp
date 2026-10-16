#include "linting.hpp"

#include "Luau/Ast.h"
#include "Luau/Linter.h"
#include "Luau/Scope.h"
#include "Luau/TxnLog.h"
#include "Luau/Type.h"
#include "Luau/TypePack.h"
#include "Luau/TypeUtils.h"
#include "instar-luau/src/bridge.rs.h"

#include <algorithm>
#include <cmath>
#include <optional>
#include <string_view>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace instar::luau {
    namespace {
        Warning diagnose(const Luau::LintWarning &warning, const std::string &module, Luau::FileResolver &resolver) {
            return Warning{
                Diagnostic{resolver.getHumanReadableModuleName(module),
                    static_cast<int32_t>(warning.code),
                    warning.location.begin.line,
                    warning.location.begin.column,
                    warning.location.end.line,
                    warning.location.end.column,
                    warning.text,
                    false},
                Luau::LintWarning::getName(warning.code),
                {},
                {},
                false,
            };
        }

        Luau::AstExpr *unwrap(Luau::AstExpr *expression) {
            while (expression) {
                if (const auto *group = expression->as<Luau::AstExprGroup>()) {
                    expression = group->expr;
                } else if (const auto *assertion = expression->as<Luau::AstExprTypeAssertion>()) {
                    expression = assertion->expr;
                } else {
                    break;
                }
            }

            return expression;
        }

        bool same(Luau::AstExpr *left, Luau::AstExpr *right) {
            left = unwrap(left);
            right = unwrap(right);

            if (const auto *local = left->as<Luau::AstExprLocal>()) {
                const auto *other = right->as<Luau::AstExprLocal>();

                return other && local->local == other->local;
            }

            const auto *global = left->as<Luau::AstExprGlobal>();
            const auto *other = right->as<Luau::AstExprGlobal>();

            return global && other && global->name == other->name;
        }

        std::string path(Luau::AstExpr *expression) {
            expression = unwrap(expression);

            if (const auto *global = expression->as<Luau::AstExprGlobal>()) {
                return global->name.value;
            }

            if (const auto *index = expression->as<Luau::AstExprIndexName>()) {
                std::string root = path(index->expr);

                return root.empty() ? root : root + "." + index->index.value;
            }

            return {};
        }

        std::optional<double> number(Luau::AstExpr *expression) {
            expression = unwrap(expression);

            if (const auto *constant = expression->as<Luau::AstExprConstantNumber>()) {
                return constant->value;
            }

            if (const auto *unary = expression->as<Luau::AstExprUnary>();
                unary && unary->op == Luau::AstExprUnary::Op::Minus) {
                const auto value = number(unary->expr);

                return value ? std::optional<double>{-*value} : std::nullopt;
            }

            if (const auto *binary = expression->as<Luau::AstExprBinary>()) {
                const auto left = number(binary->left);
                const auto right = number(binary->right);

                if (left && right) {
                    switch (binary->op) {
                    case Luau::AstExprBinary::Add:
                        return *left + *right;

                    case Luau::AstExprBinary::Sub:
                        return *left - *right;

                    case Luau::AstExprBinary::Mul:
                        return *left * *right;

                    case Luau::AstExprBinary::Div:
                        return *left / *right;

                    case Luau::AstExprBinary::FloorDiv:
                        return std::floor(*left / *right);

                    case Luau::AstExprBinary::Mod:
                        return *left - *right * std::floor(*left / *right);

                    case Luau::AstExprBinary::Pow:
                        return std::pow(*left, *right);

                    default:
                        break;
                    }
                }
            }

            return std::nullopt;
        }

        struct Binding {
            Luau::AstExpr *value = nullptr;
            std::string category = "local";
            bool read = false;
            bool assigned = false;
            bool mutated = false;
        };

        class Bindings final : public Luau::AstVisitor {
        public:
            bool visit(Luau::AstExprLocal *expression) override {
                locals[expression->local].read = true;

                return true;
            }

            bool visit(Luau::AstStatLocal *statement) override {
                for (size_t index = 0; index < statement->vars.size; ++index) {
                    auto &binding = locals[statement->vars.data[index]];
                    binding.value = index < statement->values.size ? statement->values.data[index] : nullptr;
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *statement) override {
                locals[statement->name].value = statement->func;

                return true;
            }

            bool visit(Luau::AstExprFunction *expression) override {
                for (auto *argument : expression->args) {
                    locals[argument].category = "parameter";
                }

                if (expression->self) {
                    locals[expression->self].category = "parameter";
                }

                return true;
            }

            bool visit(Luau::AstStatFor *statement) override {
                locals[statement->var].category = "loop";

                return true;
            }

            bool visit(Luau::AstStatForIn *statement) override {
                for (auto *variable : statement->vars) {
                    locals[variable].category = "loop";
                }

                return true;
            }

            bool visit(Luau::AstStatAssign *statement) override {
                for (auto *value : statement->values) {
                    value->visit(this);
                }

                for (auto *target : statement->vars) {
                    write(target, false);
                }

                return false;
            }

            bool visit(Luau::AstStatCompoundAssign *statement) override {
                write(statement->var, true);
                statement->value->visit(this);

                return false;
            }

            bool visit(Luau::AstStatFunction *statement) override {
                write(statement->name, false);
                statement->func->visit(this);

                return false;
            }

            bool visit(Luau::AstExprCall *expression) override {
                for (auto *argument : expression->args) {
                    mutate(argument);
                }

                if (expression->self) {
                    if (auto *index = expression->func->as<Luau::AstExprIndexName>()) {
                        mutate(index->expr);
                    }
                }

                return true;
            }

            void write(Luau::AstExpr *expression, bool read) {
                if (auto *local = expression->as<Luau::AstExprLocal>()) {
                    auto &binding = locals[local->local];
                    binding.assigned = true;
                    binding.read |= read;
                } else {
                    if (auto *global = expression->as<Luau::AstExprGlobal>()) {
                        globals.insert(global->name.value);
                    } else if (auto *index = expression->as<Luau::AstExprIndexName>()) {
                        mutate(index->expr);
                        globals.insert(path(index->expr));
                    } else if (auto *index = expression->as<Luau::AstExprIndexExpr>()) {
                        mutate(index->expr);
                        globals.insert(path(index->expr));
                    }

                    expression->visit(this);
                }
            }

            void mutate(Luau::AstExpr *expression) {
                std::unordered_set<Luau::AstLocal *> visited;

                while (auto *local = unwrap(expression)->as<Luau::AstExprLocal>()) {
                    if (!visited.insert(local->local).second) {
                        break;
                    }

                    auto &binding = locals[local->local];
                    binding.mutated = true;

                    if (!binding.value) {
                        break;
                    }

                    expression = binding.value;
                }
            }

            Luau::AstExpr *value(Luau::AstExpr *expression) const {
                std::unordered_set<Luau::AstLocal *> visited;
                expression = unwrap(expression);

                while (auto *local = expression->as<Luau::AstExprLocal>()) {
                    const auto found = locals.find(local->local);

                    if (!visited.insert(local->local).second || found == locals.end() || found->second.assigned ||
                        !found->second.value) {
                        break;
                    }

                    expression = unwrap(found->second.value);
                }

                return expression;
            }

            std::unordered_map<Luau::AstLocal *, Binding> locals;
            std::unordered_set<std::string> globals;
        };

        class Lints final : public Luau::AstVisitor {
        public:
            Lints(
                const Luau::SourceModule &source, const Luau::Module &module, Luau::FileResolver &resolver,
                const Bindings &bindings
            )
                : source(source), module(module), resolver(resolver), bindings(bindings) {}

            void emit(
                const Luau::Location &location, const char *rule, const std::string &message,
                const std::string &subject = {}, const std::string &category = {}, bool fixable = false
            ) {
                findings.push_back(
                    Warning{
                        Diagnostic{resolver.getHumanReadableModuleName(source.name),
                            0,
                            location.begin.line,
                            location.begin.column,
                            location.end.line,
                            location.end.column,
                            message,
                            false},
                        rule,
                        subject,
                        category,
                        fixable,
                    }
                );
            }

            bool global(Luau::AstExpr *expression, std::string_view name) const {
                return path(expression) == name && bindings.globals.count(std::string(name)) == 0 &&
                       bindings.globals.count("_G") == 0;
            }

            bool declared(Luau::AstName name) const {
                if (module.scopes.empty()) {
                    return false;
                }

                const auto &scope = module.scopes.front().second;

                return module.declaredGlobals.count(name.value) != 0 ||
                       (scope->parent && scope->parent->lookup(Luau::Symbol{name}).has_value());
            }

            bool visit(Luau::AstExprGlobal *expression) override {
                emit(expression->location, "global_usage", "access to a global binding", expression->name.value);
                emit(expression->location, "restricted_globals", "", expression->name.value);
                emit(expression->location, "deprecated", "", expression->name.value);

                return true;
            }

            bool visit(Luau::AstExprIndexName *expression) override {
                const std::string name = path(expression);

                if (!name.empty()) {
                    emit(expression->location, "deprecated", "", name);
                } else if (expression->op == ':') {
                    emit(expression->location, "deprecated", "", expression->index.value, "method");
                }

                return true;
            }

            void unscoped(Luau::AstExpr *expression) {
                if (auto *global = expression->as<Luau::AstExprGlobal>(); global && !declared(global->name)) {
                    emit(expression->location, "unscoped_variables", "assignment creates an undeclared global");
                }
            }

            bool visit(Luau::AstStatFunction *statement) override {
                unscoped(statement->name);

                return true;
            }

            bool visit(Luau::AstStatAssign *statement) override {
                for (size_t index = 0; index < statement->vars.size; ++index) {
                    auto *target = statement->vars.data[index];
                    unscoped(target);

                    if (index < statement->values.size && same(target, statement->values.data[index])) {
                        bool fixable = statement->vars.size == 1 && statement->values.size == 1 &&
                                       target->is<Luau::AstExprLocal>();

                        for (const auto &comment : source.commentLocations) {
                            if (comment.location.begin < statement->location.end &&
                                statement->location.begin < comment.location.end) {
                                fixable = false;
                            }
                        }

                        emit(
                            fixable ? statement->location : target->location,
                            "self_assignment",
                            "assignment leaves a value unchanged",
                            {},
                            {},
                            fixable
                        );
                    }
                }

                if (!loops.empty() && statement->vars.size == 1 && statement->values.size == 1) {
                    auto *binary = unwrap(statement->values.data[0])->as<Luau::AstExprBinary>();

                    if (binary && binary->op == Luau::AstExprBinary::Concat &&
                        same(statement->vars.data[0], binary->left)) {
                        emit(
                            statement->location,
                            "string_concat_in_loop",
                            "repeated concatenation grows a string quadratically"
                        );
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatCompoundAssign *statement) override {
                unscoped(statement->var);

                if (!loops.empty() && statement->op == Luau::AstExprBinary::Concat) {
                    emit(
                        statement->location,
                        "string_concat_in_loop",
                        "repeated concatenation grows a string quadratically"
                    );
                }

                return true;
            }

            bool visit(Luau::AstStatBlock *block) override {
                for (size_t index = 1; index < block->body.size; ++index) {
                    const auto *first = block->body.data[index - 1]->as<Luau::AstStatAssign>();
                    const auto *second = block->body.data[index]->as<Luau::AstStatAssign>();

                    if (first && second && first->vars.size == 1 && first->values.size == 1 && second->vars.size == 1 &&
                        second->values.size == 1 && same(first->vars.data[0], second->values.data[0]) &&
                        same(first->values.data[0], second->vars.data[0]) &&
                        !same(first->vars.data[0], first->values.data[0])) {
                        emit(
                            second->location,
                            "almost_swapped",
                            "sequential assignments overwrite a value before swapping it"
                        );
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatIf *statement) override {
                if (statement->thenbody->body.size == 1 && statement->elsebody) {
                    const auto *otherwise = statement->elsebody->as<Luau::AstStatBlock>();
                    const auto *first = statement->thenbody->body.data[0]->as<Luau::AstStatAssign>();

                    const auto *second = otherwise && otherwise->body.size == 1
                                             ? otherwise->body.data[0]->as<Luau::AstStatAssign>()
                                             : nullptr;

                    if (first && second && first->vars.size == 1 && first->values.size == 1 && second->vars.size == 1 &&
                        second->values.size == 1 && first->vars.data[0]->is<Luau::AstExprLocal>() &&
                        same(first->vars.data[0], second->vars.data[0])) {
                        emit(
                            statement->location,
                            "if_expression_assignment",
                            "matching branch assignments can use an if expression"
                        );
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *expression) override {
                auto previous = std::move(loops);
                loops.clear();
                expression->body->visit(this);
                loops = std::move(previous);

                return false;
            }

            void loop(Luau::AstStatBlock *body) {
                loops.push_back(body);
                body->visit(this);
                loops.pop_back();
            }

            bool visit(Luau::AstStatWhile *statement) override {
                statement->condition->visit(this);
                loop(statement->body);

                return false;
            }

            bool visit(Luau::AstStatRepeat *statement) override {
                loop(statement->body);
                statement->condition->visit(this);

                return false;
            }

            bool visit(Luau::AstStatFor *statement) override {
                statement->from->visit(this);
                statement->to->visit(this);

                if (statement->step) {
                    statement->step->visit(this);
                }

                loop(statement->body);

                return false;
            }

            bool visit(Luau::AstStatForIn *statement) override {
                if (statement->vars.size == 2 && statement->values.size == 1 && statement->body->body.size == 1) {
                    const auto *call = unwrap(statement->values.data[0])->as<Luau::AstExprCall>();
                    const auto *assignment = statement->body->body.data[0]->as<Luau::AstStatAssign>();

                    if (call && global(call->func, "pairs") && call->args.size == 1 && assignment &&
                        assignment->vars.size == 1 && assignment->values.size == 1) {
                        const auto *index = assignment->vars.data[0]->as<Luau::AstExprIndexExpr>();
                        const auto *value = assignment->values.data[0]->as<Luau::AstExprLocal>();
                        const auto *key = index ? index->index->as<Luau::AstExprLocal>() : nullptr;
                        const auto *destination = index ? index->expr->as<Luau::AstExprLocal>() : nullptr;

                        if (key && value && destination && key->local == statement->vars.data[0] &&
                            value->local == statement->vars.data[1]) {
                            const auto found = bindings.locals.find(destination->local);

                            const auto *table = found != bindings.locals.end() && found->second.value
                                                    ? unwrap(found->second.value)->as<Luau::AstExprTable>()
                                                    : nullptr;

                            if (table && table->items.size == 0 && !found->second.assigned) {
                                emit(statement->location, "manual_table_clone", "table-copy loop can use table.clone");
                            }
                        }
                    }
                }

                for (auto *value : statement->values) {
                    value->visit(this);
                }

                loop(statement->body);

                return false;
            }

            bool visit(Luau::AstStatExpr *statement) override {
                const auto *call = unwrap(statement->expr)->as<Luau::AstExprCall>();

                if (call) {
                    if (global(call->func, "pcall") || global(call->func, "xpcall")) {
                        emit(call->location, "ignored_pcall_result", "protected-call result is discarded");
                    }

                    const std::string name = path(call->func);

                    static const std::unordered_set<std::string> result_calls{
                        "math.abs",
                        "math.floor",
                        "math.ceil",
                        "math.sqrt",
                        "math.max",
                        "math.min",
                        "math.sin",
                        "math.cos",
                        "math.tan",
                        "math.log",
                        "math.exp",
                        "string.lower",
                        "string.upper",
                        "string.sub",
                        "string.len",
                        "string.rep",
                        "string.reverse",
                        "string.format",
                        "table.clone",
                        "table.concat",
                        "table.find",
                    };

                    if (result_calls.count(name) && bindings.globals.count(name.substr(0, name.find('.'))) == 0) {
                        emit(call->location, "must_use", "result of a result-producing function is discarded");
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprCall *call) override {
                const std::string name = path(call->func);

                if ((global(call->func, "type") || global(call->func, "typeof")) && call->args.size == 1) {
                    const auto *binary = unwrap(call->args.data[0])->as<Luau::AstExprBinary>();

                    if (binary && binary->op >= Luau::AstExprBinary::CompareNe &&
                        binary->op <= Luau::AstExprBinary::CompareGe) {
                        emit(call->location, "type_check_inside_call", "comparison belongs outside the type check");
                    }
                }

                if (global(call->func, "require")) {
                    Luau::AstExpr *argument = call->args.size == 1 ? bindings.value(call->args.data[0]) : nullptr;
                    const auto *literal = argument ? argument->as<Luau::AstExprConstantString>() : nullptr;

                    if (literal) {
                        emit(
                            call->args.data[0]->location,
                            "restricted_module_paths",
                            "",
                            std::string(literal->value.data, literal->value.size)
                        );
                    } else if (!argument || !instance(argument)) {
                        emit(call->location, "non_const_require", "require argument is not statically known");
                    }
                }

                argument_count(call);
                roblox(call, name);
                invariant(call, name);

                return true;
            }

            bool instance(Luau::AstExpr *expression) const {
                expression = unwrap(expression);

                if (const auto *name = expression->as<Luau::AstExprGlobal>()) {
                    return name->name == "script" || name->name == "game" || name->name == "workspace";
                }

                if (const auto *index = expression->as<Luau::AstExprIndexName>()) {
                    return instance(index->expr);
                }

                if (const auto *call = expression->as<Luau::AstExprCall>();
                    call && call->self && call->args.size >= 1) {
                    const auto *index = call->func->as<Luau::AstExprIndexName>();

                    return index &&
                           (index->index == "GetService" || index->index == "WaitForChild" ||
                               index->index == "FindFirstChild") &&
                           call->args.data[0]->is<Luau::AstExprConstantString>() && instance(index->expr);
                }

                return false;
            }

            void argument_count(Luau::AstExprCall *call) {
                const auto *found = module.astOriginalCallTypes.find(call->func);

                if (found && !Luau::get<Luau::FunctionType>(Luau::follow(*found))) {
                    found = module.astOverloadResolvedTypes.find(call);
                }

                if (!found) {
                    return;
                }

                const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*found));

                if (!function) {
                    return;
                }

                const auto [minimum, maximum] = Luau::getParameterExtents(Luau::TxnLog::empty(), function->argTypes);
                size_t count = call->args.size + size_t(call->self);
                bool expands = false;

                if (call->args.size) {
                    auto *last = call->args.data[call->args.size - 1];
                    expands = last->is<Luau::AstExprCall>() || last->is<Luau::AstExprVarargs>();

                    if (expands) {
                        if (const auto *pack = module.astTypePacks.find(last); pack && Luau::finite(*pack)) {
                            count = count - 1 + Luau::size(*pack);
                            expands = false;
                        } else {
                            --count;
                        }
                    }
                }

                if ((maximum && count > *maximum) || (!expands && count < minimum)) {
                    emit(
                        call->location,
                        "mismatched_argument_count",
                        "call argument count is incompatible with the established signature"
                    );
                }
            }

            void roblox(Luau::AstExprCall *call, const std::string &name) {
                const auto *member = call->func->as<Luau::AstExprIndexName>();

                if (!member) {
                    return;
                }

                const auto *root = unwrap(member->expr)->as<Luau::AstExprGlobal>();

                if (root && declared(root->name) && bindings.globals.count(root->name.value) == 0) {
                    if (name == "Color3.new") {
                        for (auto *argument : call->args) {
                            if (const auto value = number(argument); value && (*value < 0 || *value > 1)) {
                                emit(
                                    call->location,
                                    "roblox_incorrect_color3_new_bounds",
                                    "Color3.new channels use a 0-1 scale"
                                );

                                break;
                            }
                        }
                    } else if (name == "UDim2.new") {
                        if (call->args.size == 2) {
                            emit(
                                call->location,
                                "roblox_suspicious_udim2_new",
                                "UDim2.new needs scale and offset components for both axes"
                            );
                        } else if (call->args.size == 4) {
                            const bool scale = number(call->args.data[1]) == 0.0 && number(call->args.data[3]) == 0.0;
                            const bool offset = number(call->args.data[0]) == 0.0 && number(call->args.data[2]) == 0.0;

                            if (scale || offset) {
                                emit(
                                    call->location,
                                    "roblox_manual_fromscale_or_fromoffset",
                                    scale ? "use UDim2.fromScale when both offsets are zero"
                                          : "use UDim2.fromOffset when both scales are zero"
                                );
                            }
                        }
                    }
                }

                if (call->self && member->index == "GetChildren") {
                    const auto *service = bindings.value(member->expr)->as<Luau::AstExprCall>();
                    const auto *access = service ? service->func->as<Luau::AstExprIndexName>() : nullptr;

                    const auto *literal = service && service->args.size == 1
                                              ? service->args.data[0]->as<Luau::AstExprConstantString>()
                                              : nullptr;

                    if (access && service->self && access->index == "GetService" && global(access->expr, "game") &&
                        literal && std::string_view(literal->value.data, literal->value.size) == "Players") {
                        emit(call->location, "roblox_prefer_get_players", "use Players:GetPlayers() to select players");
                    }
                }
            }

            void invariant(Luau::AstExprCall *call, const std::string &name) {
                static const std::unordered_set<std::string> arithmetic{"math.abs",
                    "math.floor",
                    "math.ceil",
                    "math.sqrt",
                    "math.sin",
                    "math.cos",
                    "math.tan",
                    "math.exp"};

                if (loops.empty() || arithmetic.count(name) == 0 || bindings.globals.count("math") ||
                    bindings.globals.count("_G") || call->args.size != 1) {
                    return;
                }

                auto *argument = bindings.value(call->args.data[0]);
                const auto value = number(argument);

                if (!value || !std::isfinite(*value)) {
                    return;
                }

                if (const auto *local = unwrap(call->args.data[0])->as<Luau::AstExprLocal>();
                    local && !(local->local->location.end < loops.back()->location.begin)) {
                    return;
                }

                emit(call->location, "loop_invariant_call", "deterministic numeric call has loop-invariant arguments");
            }

            void finish() {
                std::unordered_map<Luau::AstLocal *, Luau::TypeId> inferred;

                for (const auto &[location, scope] : module.scopes) {
                    for (const auto &[symbol, binding] : scope->bindings) {
                        if (symbol.local) {
                            inferred[symbol.local] = binding.typeId;
                        }
                    }
                }

                for (const auto &[local, binding] : bindings.locals) {
                    if (!binding.read && !local->isExported) {
                        emit(
                            local->location,
                            "unused_variable",
                            "binding is never read",
                            local->name.value,
                            binding.category
                        );
                    }

                    if (binding.value && !binding.assigned && !local->isConst && binding.category == "local") {
                        const bool table = unwrap(binding.value)->is<Luau::AstExprTable>();

                        emit(
                            local->location,
                            "prefer_const",
                            "unchanged local binding can be declared const",
                            local->name.value,
                            table && binding.mutated ? "mutated_table" : "local"
                        );
                    }

                    if (!local->annotation && (binding.category == "local" || binding.category == "parameter")) {
                        const auto found = inferred.find(local);

                        if (found != inferred.end() && Luau::get<Luau::AnyType>(Luau::follow(found->second))) {
                            emit(
                                local->location,
                                binding.category == "parameter" ? "implicit_any_parameter" : "implicit_any_local",
                                "unannotated binding has inferred any type"
                            );
                        }
                    }

                    const auto *call = binding.value ? unwrap(binding.value)->as<Luau::AstExprCall>() : nullptr;

                    if (!binding.read && call && (global(call->func, "pcall") || global(call->func, "xpcall"))) {
                        emit(local->location, "ignored_pcall_result", "protected-call success status is never read");
                    }
                }
            }

            rust::Vec<Warning> findings;

        private:
            const Luau::SourceModule &source;
            const Luau::Module &module;
            Luau::FileResolver &resolver;
            const Bindings &bindings;
            std::vector<Luau::AstStatBlock *> loops;
        };
    }

    rust::Vec<Warning> lint(
        const Luau::SourceModule &source, const Luau::Module &module, Luau::FileResolver &resolver,
        const Luau::ScopePtr &globals
    ) {
        if (!source.root || !source.parseErrors.empty()) {
            return {};
        }

        Bindings bindings;
        source.root->visit(&bindings);
        Lints lints(source, module, resolver, bindings);
        source.root->visit(&lints);
        lints.finish();

        const auto &environment = module.scopes.empty() ? globals : module.scopes.front().second->parent;
        Luau::LintOptions options{~Luau::LintWarning::parseMask(source.hotcomments)};

        for (const auto &warning :
            Luau::lint(source.root, *source.names, environment, &module, source.hotcomments, options)) {
            lints.findings.push_back(diagnose(warning, source.name, resolver));
        }

        return std::move(lints.findings);
    }

}
