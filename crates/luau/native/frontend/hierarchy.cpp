#include "diagnostics.hpp"
#include "hierarchy.hpp"
#include "navigation.hpp"
#include "state.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "instar-bridge/src/boundary.rs.h"

#include <algorithm>
#include <cstddef>
#include <map>
#include <optional>
#include <set>
#include <stdexcept>
#include <string>
#include <tuple>
#include <utility>
#include <vector>

namespace instar::frontend {
    namespace {
        enum class Kind : uint8_t { Function, Class, Module };

        using Identity = std::tuple<Kind, std::string, size_t, size_t, std::string>;

        struct Pending {
            size_t caller;
            Luau::TypeId type;
            NativeLocation location;
        };

        struct Graph {
            const Host &host;
            NativeHierarchy result;
            std::map<Identity, size_t> identities;
            std::map<std::string, NativeDocument> documents;
            std::map<const Luau::ExternType *, std::optional<size_t>> classes;
            std::set<std::pair<size_t, size_t>> inheritance;
            std::set<const Luau::Scope *> scopes;
            std::vector<Pending> pending;

            explicit Graph(const Host &host) : host(host) {}

            size_t module(const std::string &name) {
                const auto &source = document(name);

                if (!source.found) {
                    throw std::runtime_error("hierarchy source is unavailable");
                }

                const auto index = result.items.size();

                result.items.push_back(
                    NativeHierarchyItem{rust::String(name),
                        static_cast<uint8_t>(Kind::Module),
                        NativeLocation{rust::String(name), source.revision, 0, source.text.size()},
                        0,
                        0}
                );

                return index;
            }

            size_t function(const std::string &name, const Luau::AstExprFunction &node, const Luau::Module &checked) {
                auto range = location(host, name, node.location);
                const Identity identity{Kind::Function, name, range.start, range.end, ""};

                if (const auto found = identities.find(identity); found != identities.end()) {
                    return found->second;
                }

                auto selection_start = range.start;
                auto selection_end = range.start;

                if (const auto *type = checked.astTypes.find(&node)) {
                    const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*type));

                    if (function && function->definition && function->definition->definitionModuleName == name &&
                        function->definition->definitionLocation == node.location) {
                        const auto selection = location(host, name, function->definition->originalNameLocation);
                        selection_start = selection.start;
                        selection_end = selection.end;
                        range.start = std::min(range.start, selection_start);
                        range.end = std::max(range.end, selection_end);
                    }
                }

                const auto index = result.items.size();
                identities.emplace(identity, index);

                result.items.push_back(
                    NativeHierarchyItem{rust::String(node.debugname.value ? node.debugname.value : "function"),
                        static_cast<uint8_t>(Kind::Function),
                        std::move(range),
                        selection_start,
                        selection_end}
                );

                return index;
            }

            void scope(Luau::ScopePtr scope) {
                for (; scope && scopes.insert(scope.get()).second; scope = scope->parent) {
                    std::map<std::string, Luau::TypeId> types;

                    for (const auto &[name, type] : scope->exportedTypeBindings) {
                        types.insert_or_assign(name, type.type);
                    }

                    for (const auto &[name, type] : scope->privateTypeBindings) {
                        types.insert_or_assign(name, type.type);
                    }

                    for (const auto &[name, type] : types) {
                        nominal(type);
                    }
                }
            }

            void call(size_t caller, Luau::TypeId type, const std::string &module, Luau::Location range) {
                pending.push_back(Pending{caller, type, location(host, module, range)});
            }

            void select(Luau::TypeId initial) {
                std::vector<Luau::TypeId> pending{initial};
                std::set<Luau::TypeId> visited;
                std::set<size_t> selected;

                while (!pending.empty()) {
                    const auto type = Luau::follow(pending.back());
                    pending.pop_back();

                    if (!visited.insert(type).second) {
                        continue;
                    }

                    if (const auto index = function_index(type)) {
                        selected.insert(*index);
                    } else if (const auto index = nominal(type)) {
                        selected.insert(*index);
                    } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                        pending.insert(pending.end(), union_type->options.begin(), union_type->options.end());
                    }
                }

                for (const auto index : selected) {
                    result.selected.push_back(index);
                }
            }

            NativeHierarchy finish() {
                for (auto &call : pending) {
                    if (const auto callee = function_index(call.type)) {
                        result.calls.push_back(NativeCall{call.caller, *callee, std::move(call.location)});
                    }
                }

                for (const auto &[child, parent] : inheritance) {
                    result.inheritance.push_back(NativeInheritance{child, parent});
                }

                return std::move(result);
            }

          private:
            const NativeDocument &document(const std::string &name) {
                auto found = documents.find(name);

                if (found == documents.end()) {
                    found = documents.emplace(name, host.read_document(name)).first;
                }

                return found->second;
            }

            std::optional<size_t> function_index(Luau::TypeId initial) const {
                std::vector<Luau::TypeId> pending{initial};
                std::set<Luau::TypeId> visited;
                std::optional<size_t> result;

                while (!pending.empty()) {
                    const auto type = Luau::follow(pending.back());
                    pending.pop_back();

                    if (!visited.insert(type).second) {
                        continue;
                    }

                    if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                        pending.insert(pending.end(), intersection->parts.begin(), intersection->parts.end());
                        continue;
                    }

                    const auto *function = Luau::get<Luau::FunctionType>(type);

                    if (!function || !function->definition || !function->definition->definitionModuleName) {
                        return std::nullopt;
                    }

                    const auto &module = *function->definition->definitionModuleName;
                    const auto source = documents.find(module);

                    if (source == documents.end() || !source->second.found) {
                        return std::nullopt;
                    }

                    const auto range = location(host, module, function->definition->definitionLocation);
                    const auto found = identities.find(Identity{Kind::Function, module, range.start, range.end, ""});

                    if (found == identities.end() || (result && *result != found->second)) {
                        return std::nullopt;
                    }

                    result = found->second;
                }

                return result;
            }

            std::optional<size_t> nominal(Luau::TypeId initial) {
                std::optional<size_t> first;
                std::optional<size_t> previous;
                auto type = initial;

                while (const auto *external = Luau::get<Luau::ExternType>(Luau::follow(type))) {
                    if (const auto found = classes.find(external); found != classes.end()) {
                        if (previous && found->second && *previous != *found->second) {
                            inheritance.emplace(*previous, *found->second);
                        }

                        return first ? first : found->second;
                    }

                    const auto &module = external->definitionModuleName;

                    if (!external->definitionLocation ||
                        external->definitionLocation->begin == external->definitionLocation->end || module.empty() ||
                        !document(module).found) {
                        classes.emplace(external, std::nullopt);
                        break;
                    }

                    auto range = location(host, module, *external->definitionLocation);
                    const Identity identity{Kind::Class, module, range.start, range.end, external->name};
                    auto found = identities.find(identity);

                    if (found == identities.end()) {
                        const auto index = result.items.size();
                        found = identities.emplace(identity, index).first;
                        const auto selection = range.start;

                        result.items.push_back(
                            NativeHierarchyItem{rust::String(external->name),
                                static_cast<uint8_t>(Kind::Class),
                                std::move(range),
                                selection,
                                selection}
                        );
                    }

                    const auto index = found->second;
                    classes.emplace(external, index);

                    if (!first) {
                        first = index;
                    }

                    if (previous && *previous != index) {
                        inheritance.emplace(*previous, index);
                    }

                    previous = index;

                    if (!external->parent) {
                        break;
                    }

                    type = *external->parent;
                }

                return first;
            }
        };

        struct Collector final : Luau::AstVisitor {
            Graph &graph;
            const std::string &name;
            const Luau::Module &checked;
            size_t caller;

            Collector(Graph &graph, const std::string &name, const Luau::Module &checked)
                : graph(graph), name(name), checked(checked), caller(graph.module(name)) {}

            bool visit(Luau::AstExprFunction *node) override {
                const auto previous = caller;
                caller = graph.function(name, *node, checked);
                node->body->visit(this);
                caller = previous;

                return false;
            }

            bool visit(Luau::AstExprCall *node) override {
                const auto *type = checked.astOriginalCallTypes.find(node);
                const auto *selected = checked.astOverloadResolvedTypes.find(node);

                if (selected) {
                    const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*selected));

                    if (function && function->definition) {
                        type = selected;
                    }
                }

                if (!type) {
                    type = checked.astTypes.find(node->func);
                }

                if (type) {
                    graph.call(caller, *type, name, node->func->location);
                }

                return true;
            }
        };
    }

    NativeHierarchy hierarchy(
        State &state, const Host &host, rust::Slice<const rust::String> modules, rust::Str module,
        rust::Slice<const uint32_t> position
    ) {
        if (!position.empty() && position.size() != 2) {
            throw std::runtime_error("invalid hierarchy position");
        }

        Graph graph(host);

        for (const auto &module : modules) {
            const std::string name(module.data(), module.size());
            const auto *source = state.frontend.getSourceModule(name);
            const auto checked = state.frontend.moduleResolver.getModule(name);

            if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
                throw std::runtime_error("semantic module is unavailable");
            }

            Collector collector(graph, name, *checked);
            source->root->visit(&collector);

            for (const auto &[range, scope] : checked->scopes) {
                graph.scope(scope);
            }
        }

        if (!position.empty()) {
            const std::string name(module.data(), module.size());
            const auto *source = state.frontend.getSourceModule(name);
            const auto checked = state.frontend.moduleResolver.getModule(name);

            if (!source || !checked) {
                throw std::runtime_error("hierarchy entry is unavailable");
            }

            if (const auto type = inferred_type(*checked, *source, Luau::Position(position[0], position[1]))) {
                graph.select(*type);
            }
        }

        return graph.finish();
    }
}
