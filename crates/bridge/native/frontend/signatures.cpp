#include "diagnostics.hpp"
#include "signatures.hpp"
#include "state.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Lexer.h"
#include "Luau/Module.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"
#include "Luau/TypePack.h"
#include "instar-bridge/src/boundary.rs.h"

#include <algorithm>
#include <optional>
#include <set>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace instar::frontend {
    namespace {
        const Luau::AstExprCall *call_at(const Luau::SourceModule &source, Luau::Position position) {
            const auto ancestry = Luau::findAncestryAtPositionForAutocomplete(source, position);

            for (auto node = ancestry.rbegin(); node != ancestry.rend(); ++node) {
                if ((*node)->is<Luau::AstExprFunction>()) {
                    return nullptr;
                }

                if (const auto *call = (*node)->as<Luau::AstExprCall>();
                    call && call->argLocation.containsClosed(position)) {
                    return call;
                }
            }

            return nullptr;
        }

        size_t argument(const NativeDocument &document, const Luau::AstExprCall &call, Luau::Position position) {
            Luau::Allocator allocator;
            Luau::AstNameTable names(allocator);
            Luau::Lexer lexer(document.text.data(), document.text.size(), names);
            size_t result = 0;

            for (auto token = lexer.next(); token.type != Luau::Lexeme::Eof && token.location.end <= position;
                token = lexer.next()) {
                if (token.type != ',' || token.location.begin < call.argLocation.begin) {
                    continue;
                }

                bool nested = false;

                for (const auto *value : call.args) {
                    if (value->location.contains(token.location.begin)) {
                        nested = true;
                        break;
                    }
                }

                if (!nested) {
                    ++result;
                }
            }

            return result;
        }

        rust::Vec<NativeParameter> parameters(const std::string &label) {
            Luau::Allocator allocator;
            Luau::AstNameTable names(allocator);
            Luau::Lexer lexer(label.data(), label.size(), names);
            rust::Vec<NativeParameter> result;
            size_t depth = 0;
            std::optional<size_t> start;
            size_t end = 0;

            for (auto token = lexer.next(); token.type != Luau::Lexeme::Eof; token = lexer.next()) {
                if (depth == 0) {
                    if (token.type == '(') {
                        depth = 1;
                    }

                    continue;
                }

                if (depth == 1 && (token.type == ',' || token.type == ')')) {
                    if (start) {
                        result.push_back(NativeParameter{*start, end});
                        start.reset();
                    }

                    if (token.type == ')') {
                        return result;
                    }

                    continue;
                }

                if (!start) {
                    start = token.location.begin.column;
                }

                end = token.location.end.column;

                if (token.type == '(' || token.type == '{' || token.type == '[' || token.type == '<') {
                    ++depth;
                } else if (token.type == ')' || token.type == '}' || token.type == ']' || token.type == '>') {
                    --depth;
                }
            }

            throw std::runtime_error("native signature label is incomplete");
        }

        NativeSignature detach(
            const std::string &name, const Luau::FunctionType &function, size_t receivers,
            Luau::ToStringOptions &options
        ) {
            auto [head, tail] = Luau::flatten(function.argTypes);
            const auto skip = std::min(receivers, head.size());
            head.erase(head.begin(), head.begin() + skip);
            Luau::TypePackVar arguments{Luau::TypePack{std::move(head), tail}};
            auto display = function;
            display.argTypes = &arguments;

            display.argNames.erase(
                display.argNames.begin(),
                display.argNames.begin() + std::min(skip, display.argNames.size())
            );

            const auto label = Luau::toStringNamedFunction(name, display, options);
            const auto *variadic = tail ? Luau::get<Luau::VariadicTypePack>(Luau::follow(*tail)) : nullptr;

            return NativeSignature{rust::String(label),
                parameters(label),
                tail.has_value() && (!variadic || !variadic->hidden)};
        }

        std::vector<Luau::TypeId> functions(Luau::TypeId initial) {
            std::vector<Luau::TypeId> result;
            std::vector<Luau::TypeId> pending{initial};
            std::set<Luau::TypeId> seen;

            while (!pending.empty()) {
                const auto type = Luau::follow(pending.back());
                pending.pop_back();

                if (!seen.insert(type).second) {
                    continue;
                }

                if (Luau::get<Luau::FunctionType>(type)) {
                    result.push_back(type);
                } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                    pending.insert(pending.end(), intersection->parts.rbegin(), intersection->parts.rend());
                }
            }

            return result;
        }
    }

    NativeSignatures signatures(State &state, const Host &host, rust::Str module, uint32_t line, uint32_t column) {
        const std::string name(module.data(), module.size());
        const auto *source = state.frontend.getSourceModule(name);
        const auto checked = state.frontend.moduleResolver.getModule(name);

        if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
            throw std::runtime_error("semantic module is unavailable");
        }

        const Luau::Position position(line, column);
        const auto *call = call_at(*source, position);

        if (!call) {
            return {};
        }

        const auto *original = checked->astOriginalCallTypes.find(call);
        const auto *callee = checked->astTypes.find(call->func);

        if (!original) {
            original = callee;
        }

        if (!original) {
            return {};
        }

        auto candidates = functions(*original);
        const auto *selected = checked->astOverloadResolvedTypes.find(call);
        const auto selection = selected ? Luau::follow(*selected) : nullptr;

        if (selection && Luau::get<Luau::FunctionType>(selection) &&
            std::find(candidates.begin(), candidates.end(), selection) == candidates.end()) {
            candidates.push_back(selection);
        }

        if (candidates.empty()) {
            return {};
        }

        size_t receivers = call->self ? 1 : 0;

        if (callee) {
            const auto type = Luau::follow(*callee);

            if (Luau::get<Luau::MetatableType>(type) || Luau::get<Luau::TableType>(type) ||
                Luau::get<Luau::ExternType>(type)) {
                ++receivers;
            }
        }

        Luau::ToStringOptions options;
        options.functionTypeArguments = true;
        options.scope = Luau::findScopeAtPosition(*checked, position);
        const auto identifier = Luau::getIdentifier(call->func);
        auto documentation_position = call->func->location.begin;

        if (const auto *member = call->func->as<Luau::AstExprIndexName>()) {
            documentation_position = member->indexLocation.begin;
        }

        NativeSignatures result{};
        result.found = true;
        result.location = location(host, name, call->location);
        result.argument = argument(host.read_document(name), *call, position);

        result.documentation = rust::String(
            Luau::getDocumentationSymbolAtPosition(*source, *checked, documentation_position).value_or("")
        );

        for (const auto type : candidates) {
            if (type == selection || candidates.size() == 1) {
                result.active = result.entries.size();
                result.has_active = true;
            }

            result.entries.push_back(detach(
                identifier.value ? identifier.value : "function",
                *Luau::get<Luau::FunctionType>(type),
                receivers,
                options
            ));
        }

        return result;
    }
}
