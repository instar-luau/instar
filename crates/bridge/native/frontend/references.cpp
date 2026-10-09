#include "diagnostics.hpp"
#include "navigation.hpp"
#include "references.hpp"
#include "state.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "instar-bridge/src/boundary.rs.h"

#include <algorithm>
#include <cstdint>
#include <map>
#include <set>
#include <stdexcept>
#include <string>
#include <utility>

namespace instar::frontend {
    namespace {
        enum class Category : uint8_t { Binding, Type, Property, Receiver };

        struct References final : Luau::AstVisitor {
            State &state;
            const Host &host;
            const std::string &name;
            const Luau::SourceModule &source;
            const Luau::Module &checked;
            rust::Vec<NativeReference> &result;
            std::set<const Luau::AstExpr *> writes;
            std::map<const Luau::AstLocal *, Luau::Location> receivers;
            std::map<const Luau::AstLocal *, Luau::TypeId> bindings;
            std::set<const Luau::AstLocal *> parameters;

            References(
                State &state, const Host &host, const std::string &name, const Luau::SourceModule &source,
                const Luau::Module &checked, rust::Vec<NativeReference> &result
            )
                : state(state), host(host), name(name), source(source), checked(checked), result(result) {
                for (const auto &entry : checked.scopes) {
                    for (const auto &[symbol, binding] : entry.second->bindings) {
                        if (symbol.local) {
                            bindings.try_emplace(symbol.local, binding.typeId);
                        }
                    }
                }
            }

            bool visit(Luau::AstStatLocal *node) override {
                for (const auto *binding : node->vars) {
                    local(*binding);
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *node) override {
                local(*node->name);

                return true;
            }

            bool visit(Luau::AstStatFor *node) override {
                local(*node->var);

                return true;
            }

            bool visit(Luau::AstStatForIn *node) override {
                for (const auto *binding : node->vars) {
                    local(*binding);
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *node) override {
                if (node->self) {
                    receivers.insert_or_assign(node->self, node->location);
                }

                for (const auto *binding : node->args) {
                    parameters.insert(binding);
                    local(*binding);
                }

                return true;
            }

            bool visit(Luau::AstStatTypeAlias *node) override {
                declared(node->name.value, node->nameLocation, Category::Type);

                return true;
            }

            bool visit(Luau::AstExprLocal *node) override {
                const auto receiver = receivers.find(node->local);
                rust::Vec<NativeLocation> targets;

                targets.push_back(
                    location(host, name, receiver == receivers.end() ? node->local->location : receiver->second)
                );

                append(
                    node->local->name.value,
                    node->location,
                    receiver == receivers.end() ? Category::Binding : Category::Receiver,
                    std::move(targets),
                    false,
                    writes.find(node) != writes.end(),
                    false,
                    expression_type(node),
                    parameters.find(node->local) != parameters.end()
                );

                return true;
            }

            bool visit(Luau::AstExprGlobal *node) override {
                const auto position = node->location.begin;
                auto targets = definition(state, host, name, position.line, position.column, false);
                const auto binding = checked.getModuleScope()->lookupEx(Luau::Symbol(node->name));

                append(
                    node->name.value,
                    node->location,
                    Category::Binding,
                    std::move(targets),
                    false,
                    writes.find(node) != writes.end(),
                    false,
                    expression_type(node),
                    false,
                    binding && binding->first->deprecated
                );

                return true;
            }

            bool visit(Luau::AstExprIndexName *node) override {
                if (const auto *type = checked.astTypes.find(node->expr)) {
                    member(
                        *type,
                        node->index.value,
                        node->indexLocation,
                        false,
                        writes.find(node) != writes.end(),
                        false,
                        expression_type(node)
                    );
                }

                return true;
            }

            bool visit(Luau::AstExprIndexExpr *node) override {
                const auto *key = node->index->as<Luau::AstExprConstantString>();
                const auto *type = checked.astTypes.find(node->expr);

                if (key && type) {
                    member(
                        *type,
                        std::string(key->value.data, key->value.size),
                        key->location,
                        false,
                        writes.find(node) != writes.end(),
                        true,
                        expression_type(node)
                    );
                }

                return true;
            }

            bool visit(Luau::AstExprTable *node) override {
                const auto *type = checked.astTypes.find(node);

                if (!type) {
                    return true;
                }

                for (const auto &item : node->items) {
                    if (const auto *key = item.key ? item.key->as<Luau::AstExprConstantString>() : nullptr) {
                        member(
                            *type,
                            std::string(key->value.data, key->value.size),
                            key->location,
                            true,
                            true,
                            item.kind == Luau::AstExprTable::Item::Kind::General,
                            expression_type(item.value)
                        );
                    }
                }

                return true;
            }

            bool visit(Luau::AstType *node) override { return true; }

            bool visit(Luau::AstTypePack *node) override { return true; }

            bool visit(Luau::AstTypeReference *node) override {
                if (node->prefixLocal && node->prefixLocation) {
                    rust::Vec<NativeLocation> targets;
                    targets.push_back(location(host, name, node->prefixLocal->location));

                    append(
                        node->prefixLocal->name.value,
                        *node->prefixLocation,
                        Category::Binding,
                        std::move(targets),
                        false,
                        false
                    );
                }

                const auto position = node->nameLocation.begin;
                auto targets = definition(state, host, name, position.line, position.column, false);
                append(node->name.value, node->nameLocation, Category::Type, std::move(targets), false, false);

                return true;
            }

            bool visit(Luau::AstTypeTable *node) override {
                if (const auto *type = checked.astResolvedTypes.find(node)) {
                    for (const auto &field : node->props) {
                        const auto *value = checked.astResolvedTypes.find(field.type);
                        member(*type, field.name.value, field.location, true, true, false, value ? *value : nullptr);
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatAssign *node) override {
                writes.insert(node->vars.begin(), node->vars.end());

                return true;
            }

            bool visit(Luau::AstStatCompoundAssign *node) override {
                writes.insert(node->var);

                return true;
            }

            bool visit(Luau::AstStatFunction *node) override {
                writes.insert(node->name);

                return true;
            }

          private:
            void local(const Luau::AstLocal &binding) {
                const auto type = bindings.find(&binding);

                declared(
                    binding.name.value,
                    binding.location,
                    Category::Binding,
                    type == bindings.end() ? nullptr : type->second,
                    parameters.find(&binding) != parameters.end()
                );
            }

            void declared(
                const std::string &label, Luau::Location range, Category category, Luau::TypeId type = nullptr,
                bool parameter = false
            ) {
                rust::Vec<NativeLocation> targets;
                targets.push_back(location(host, name, range));
                append(label, range, category, std::move(targets), true, true, false, type, parameter);
            }

            void member(
                Luau::TypeId type, const std::string &label, Luau::Location range, bool declaration, bool writing,
                bool literal, Luau::TypeId value = nullptr
            ) {
                rust::Vec<NativeLocation> targets;
                property(host, type, label, targets);
                append(label, range, Category::Property, std::move(targets), declaration, writing, literal, value);
            }

            void append(
                const std::string &label, Luau::Location range, Category category, rust::Vec<NativeLocation> targets,
                bool declaration, bool writing, bool literal = false, Luau::TypeId type = nullptr,
                bool parameter = false, bool deprecated = false
            ) {
                if (range.begin == range.end) {
                    return;
                }

                const auto reference = location(host, name, range);

                const auto documentation =
                    targets.empty() ? Luau::getDocumentationSymbolAtPosition(source, checked, range.begin).value_or("")
                                    : std::string();

                if (targets.empty() && documentation.empty()) {
                    return;
                }

                declaration = declaration || std::any_of(targets.begin(), targets.end(), [&](const auto &target) {
                    return target.module == reference.module && target.revision == reference.revision &&
                           target.start == reference.start && target.end == reference.end;
                });

                const auto *function = type ? Luau::get<Luau::FunctionType>(Luau::follow(type)) : nullptr;
                const auto *overloads = type ? Luau::get<Luau::IntersectionType>(Luau::follow(type)) : nullptr;

                const bool callable =
                    function ||
                    (overloads && !overloads->parts.empty() &&
                        std::all_of(overloads->parts.begin(), overloads->parts.end(), [](Luau::TypeId part) {
                            return Luau::get<Luau::FunctionType>(Luau::follow(part)) != nullptr;
                        }));

                result.push_back(
                    NativeReference{reference,
                        std::move(targets),
                        rust::String(documentation),
                        rust::String(label),
                        static_cast<uint8_t>(category),
                        declaration,
                        writing,
                        literal,
                        callable,
                        parameter,
                        deprecated || (function && function->isDeprecatedFunction)}
                );
            }

            Luau::TypeId expression_type(const Luau::AstExpr *expression) const {
                if (!expression) {
                    return nullptr;
                }

                const auto *type = checked.astTypes.find(expression);

                return type ? *type : nullptr;
            }
        };
    }

    rust::Vec<NativeReference> references(State &state, const Host &host, rust::Slice<const rust::String> modules) {
        rust::Vec<NativeReference> result;

        for (const auto &module : modules) {
            const std::string name(module.data(), module.size());
            const auto *source = state.frontend.getSourceModule(name);
            const auto checked = state.frontend.moduleResolver.getModule(name);

            if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
                throw std::runtime_error("semantic module is unavailable");
            }

            References visitor(state, host, name, *source, *checked, result);
            source->root->visit(&visitor);
        }

        return result;
    }
}
