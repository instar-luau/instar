#include "analysis.hpp"
#include "completion.hpp"
#include "parsing.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Lexer.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/StringUtils.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"
#include "instar-luau/src/boundary/mod.rs.h"

#include <algorithm>
#include <map>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace instar::native {
    namespace {
        void
        prefix(const NativeDocument &document, Luau::Lexeme::Type kind, size_t cursor, NativeSuggestions &suggestions) {
            const std::string_view text(document.text.data(), document.text.size());
            const auto start = suggestions.location.start;
            const auto end = suggestions.location.end;

            if (start == end) {
                return;
            }

            auto content = start + 1;

            if (text[start] == '[') {
                while (content < end && text[content] == '=') {
                    ++content;
                }

                if (content == end || text[content] != '[') {
                    return;
                }

                ++content;
            }

            const auto finish = kind == Luau::Lexeme::BrokenString ? end : end - (content - start);

            if (cursor < content || finish < content) {
                return;
            }

            std::string value(text.substr(content, std::min(cursor, finish) - content));

            if (text[start] == '[') {
                Luau::Lexer::fixupMultilineString(value);
            } else if (!Luau::Lexer::fixupQuotedString(value)) {
                return;
            }

            suggestions.prefix = rust::String(value);
            suggestions.has_prefix = true;
        }

        void replacement(
            const NativeOperation &host, const std::string &name, Luau::Position position,
            Luau::AutocompleteContext context, NativeSuggestions &suggestions
        ) {
            const auto document = host.read_document(name);
            const auto byte = offset(host, name, position);
            suggestions.location = NativeLocation{rust::String(name), document.revision, byte, byte};

            if (context == Luau::AutocompleteContext::HotComment) {
                auto start = byte;
                auto end = byte;

                while (start > 0 && Luau::isIdentifier(std::string_view(document.text.data() + start - 1, 1))) {
                    --start;
                }

                while (
                    end < document.text.size() && Luau::isIdentifier(std::string_view(document.text.data() + end, 1))) {
                    ++end;
                }

                suggestions.location = NativeLocation{rust::String(name), document.revision, start, end};

                return;
            }

            Luau::Allocator allocator;
            Luau::AstNameTable names(allocator);
            Luau::Lexer lexer(document.text.data(), document.text.size(), names);

            for (auto token = lexer.next(); token.type != Luau::Lexeme::Eof && token.location.begin <= position;
                token = lexer.next()) {
                if (!token.location.containsClosed(position)) {
                    continue;
                }

                const bool word = token.type == Luau::Lexeme::Name || (token.type >= Luau::Lexeme::Reserved_BEGIN &&
                                                                          token.type < Luau::Lexeme::Reserved_END);

                const bool string = token.type == Luau::Lexeme::QuotedString || token.type == Luau::Lexeme::RawString ||
                                    token.type == Luau::Lexeme::InterpStringSimple ||
                                    token.type == Luau::Lexeme::BrokenString;

                if (string && token.location.begin < position) {
                    suggestions.location = location(host, name, token.location);
                    suggestions.context = static_cast<uint8_t>(Luau::AutocompleteContext::String);
                    prefix(document, token.type, byte, suggestions);

                    return;
                }

                if (context != Luau::AutocompleteContext::String && word) {
                    suggestions.location = location(host, name, token.location);

                    return;
                }
            }
        }

        std::optional<Luau::AutocompleteEntryMap>
        strings(const NativeOperation &host, rust::Str module, const Luau::ScopePtr &scope, const std::string &tag) {
            if (tag != "instar.class" && tag != "instar.service" && tag != "instar.creatable") {
                return std::nullopt;
            }

            Luau::AutocompleteEntryMap entries;

            for (const auto &definition : host.read_source(module).classes) {
                if ((tag == "instar.service" && !definition.service) ||
                    (tag == "instar.creatable" && !definition.creatable)) {
                    continue;
                }

                const std::string name(definition.name);
                Luau::AutocompleteEntry entry{Luau::AutocompleteEntryKind::String};

                if (scope) {
                    if (const auto type = scope->lookupType(name)) {
                        entry.type = type->type;
                        entry.documentationSymbol = type->type->documentationSymbol;
                    }
                }

                entries.emplace(name, std::move(entry));
            }

            return entries;
        }

        bool loader(Luau::TypeId type) {
            const auto parts = Luau::flattenIntersection(type);

            return std::any_of(parts.begin(), parts.end(), [](Luau::TypeId part) {
                return Luau::hasTag(part, Luau::kRequireTagName);
            });
        }

        bool require_argument(
            const Luau::Module &module, const std::vector<Luau::AstNode *> &ancestry, Luau::Position position
        ) {
            for (auto node = ancestry.rbegin(); node != ancestry.rend(); ++node) {
                const auto *call = (*node)->as<Luau::AstExprCall>();

                if (!call) {
                    continue;
                }

                if (call->args.size == 0 || !call->args.data[0]->location.containsClosed(position)) {
                    return false;
                }

                if (const auto *type = module.astTypes.find(call->func)) {
                    return loader(*type);
                }

                return false;
            }

            return false;
        }

        NativeSuggestion
        detach(const std::string &name, const Luau::AutocompleteEntry &entry, Luau::ToStringOptions &options) {
            const auto type = entry.type ? Luau::follow(*entry.type) : nullptr;
            const bool callable = type && (Luau::get<Luau::FunctionType>(type) || Luau::isOverloadedFunction(type));
            const auto *function = type ? Luau::get<Luau::FunctionType>(type) : nullptr;
            const bool deprecated = entry.deprecated || (function && function->isDeprecatedFunction);

            const bool identifier = !name.empty() && Luau::isIdentifier(name) &&
                                    !(name.front() >= '0' && name.front() <= '9') && !Luau::Lexer::isReserved(name);

            const auto notation = static_cast<uint8_t>(!identifier ? 1 : entry.replaceDotWithColon ? 2 : 0);

            const auto documentation = entry.documentationSymbol ? entry.documentationSymbol
                                       : type                    ? type->documentationSymbol
                                                                 : std::nullopt;

            return NativeSuggestion{rust::String(name),
                static_cast<uint8_t>(entry.kind),
                rust::String(type ? Luau::toString(type, options) : ""),
                rust::String(documentation.value_or("")),
                rust::String(entry.insertText.value_or("")),
                static_cast<uint8_t>(entry.parens),
                deprecated,
                callable,
                entry.typeCorrect != Luau::TypeCorrectKind::None,
                notation};
        }

        NativeSuggestions
        suggest(AnalysisState &state, const NativeOperation &host, rust::Str module, uint32_t line, uint32_t column) {
            const std::string name(module.data(), module.size());
            const auto *source = state.frontend.getSourceModule(name);
            const auto checked = state.frontend.moduleResolver.getModule(name);

            if (!source || !source->root || !checked || !checked->hasModuleScope() || checked->cancelled ||
                checked->timeout || state.frontend.isDirty(name)) {
                throw std::runtime_error("semantic module is unavailable");
            }

            const Luau::Position position(line, column);
            const auto scope = Luau::findScopeAtPosition(*checked, position);

            auto result = Luau::autocomplete(
                state.frontend,
                name,
                position,
                [&](std::string tag, std::optional<const Luau::ExternType *>, std::optional<std::string>) {
                    return strings(host, module, scope, tag);
                }
            );

            NativeSuggestions suggestions{};
            suggestions.context = static_cast<uint8_t>(result.context);
            replacement(host, name, position, result.context, suggestions);

            suggestions.require = suggestions.context == static_cast<uint8_t>(Luau::AutocompleteContext::String) &&
                                  require_argument(*checked, result.ancestry, position);

            for (auto node = result.ancestry.rbegin(); node != result.ancestry.rend(); ++node) {
                if (const auto *index = (*node)->as<Luau::AstExprIndexName>();
                    index && result.context == Luau::AutocompleteContext::Property && index->opPosition < position) {
                    suggestions.index_start = offset(host, name, index->opPosition);
                    suggestions.index_end = suggestions.index_start + 1;
                    suggestions.has_index = true;
                    break;
                }
            }

            Luau::ToStringOptions options;
            options.functionTypeArguments = true;
            options.scope = scope;
            std::map<std::string, Luau::AutocompleteEntry> ordered(result.entryMap.begin(), result.entryMap.end());

            for (const auto &[label, entry] : ordered) {
                if (suggestions.context == static_cast<uint8_t>(Luau::AutocompleteContext::String) &&
                    result.context != Luau::AutocompleteContext::String &&
                    !(result.context == Luau::AutocompleteContext::Property &&
                        entry.kind == Luau::AutocompleteEntryKind::Property)) {
                    continue;
                }

                if (entry.kind == Luau::AutocompleteEntryKind::GeneratedFunction && entry.insertText &&
                    entry.insertText->empty()) {
                    continue;
                }

                if (label == "require" && entry.kind == Luau::AutocompleteEntryKind::Binding && entry.type) {
                    suggestions.loader = loader(*entry.type);
                }

                suggestions.entries.push_back(detach(label, entry, options));
            }

            return suggestions;
        }
    }

    NativeCompletions
    complete(AnalysisState &state, const NativePositionInput &input, const NativeOperation &operation) {
        NativeCompletions result{};
        result.completion = NativeCompletion::Complete;

        if (operation.requested()) {
            result.completion = NativeCompletion::Cancelled;

            return result;
        }

        if (operation.remaining_nanoseconds() == 0) {
            result.completion = NativeCompletion::Timeout;

            return result;
        }

        if (operation.offset(input.name, input.line, input.column) != input.offset) {
            throw std::invalid_argument("native position disagrees with captured source coordinates");
        }

        result.suggestions = suggest(state, operation, input.name, input.line, input.column);

        if (operation.requested()) {
            result.completion = NativeCompletion::Cancelled;
        } else if (operation.remaining_nanoseconds() == 0) {
            result.completion = NativeCompletion::Timeout;
        }

        if (result.completion == NativeCompletion::Complete &&
            !state.outcomes.at(std::string(input.name)).graph_available) {
            result.completion = NativeCompletion::Analysis;
        }

        return result;
    }
}
