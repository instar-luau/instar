#include "query.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/Lexer.h"
#include "Luau/Parser.h"
#include "Luau/StringUtils.h"
#include "Luau/TypePack.h"
#include "types.hpp"

#include <algorithm>
#include <functional>
#include <stdexcept>
#include <unordered_set>
#include <utility>

namespace instar::luau::query {
    namespace {
        Range range(const Luau::Location &location) {
            return {{location.begin.line, location.begin.column}, {location.end.line, location.end.column}};
        }

        bool is_identifier_character(unsigned char value) {
            return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z') || (value >= '0' && value <= '9') ||
                   value == '_';
        }

        Luau::Location replacement(const std::string &source, Luau::Position position) {
            const auto cursor = offset(source, position);
            auto start = cursor;
            auto end = cursor;

            while (start > 0 && is_identifier_character(static_cast<unsigned char>(source[start - 1]))) {
                --start;
            }

            while (end < source.size() && is_identifier_character(static_cast<unsigned char>(source[end]))) {
                ++end;
            }

            return {{position.line, position.column - static_cast<unsigned>(cursor - start)},
                {position.line, position.column + static_cast<unsigned>(end - cursor)}};
        }

        Luau::Location contents(const std::string &source, Luau::Location location) {
            const auto start = offset(source, location.begin);
            const auto end = offset(source, location.end);

            if (start == end) {
                return location;
            }

            if (source[start] == '"' || source[start] == '\'') {
                ++location.begin.column;

                if (end > start + 1 && source[end - 1] == source[start]) {
                    --location.end.column;
                }
            } else if (source[start] == '[') {
                auto delimiter = start + 1;

                while (delimiter < end && source[delimiter] == '=') {
                    ++delimiter;
                }

                if (delimiter < end && source[delimiter] == '[') {
                    const auto length = delimiter - start + 1;
                    location.begin.column += static_cast<unsigned>(length);
                    const std::string closing = "]" + source.substr(start + 1, length - 2) + "]";

                    if (end >= start + 2 * length && source.compare(end - length, length, closing) == 0) {
                        location.end.column -= static_cast<unsigned>(length);
                    }
                }
            }

            return location;
        }

        bool is_callable(Luau::TypeId root, PresentationBudget &budget, bool &has_receiver) {
            std::vector<Luau::TypeId> pending{root};
            std::unordered_set<Luau::TypeId> seen;

            while (!pending.empty()) {
                budget.check();
                const auto type = pending.back();
                pending.pop_back();

                if (!seen.insert(type).second) {
                    continue;
                }

                if (const auto *bound = Luau::get_if<Luau::BoundType>(&type->ty)) {
                    pending.push_back(bound->boundTo);
                } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                    for (const auto part : intersection->parts) {
                        budget.check();
                        pending.push_back(part);
                    }
                } else if (const auto *table = Luau::get<Luau::TableType>(type); table && table->boundTo) {
                    pending.push_back(*table->boundTo);
                } else if (const auto *lazy = Luau::get<Luau::LazyType>(type)) {
                    const auto value = lazy->unwrapped.load();

                    if (!value) {
                        return false;
                    }

                    pending.push_back(value);
                } else if (const auto *function = Luau::get<Luau::FunctionType>(type)) {
                    has_receiver = has_receiver && function->hasSelf;
                } else {
                    return false;
                }
            }

            return true;
        }

        std::pair<std::vector<Luau::TypeId>, std::optional<Luau::TypePackId>>
        parameter_types(Luau::TypePackId current, PresentationBudget &budget) {
            std::vector<Luau::TypeId> head;
            std::unordered_set<Luau::TypePackId> seen;

            while (current && seen.insert(current).second) {
                budget.check();

                if (const auto *bound = Luau::get_if<Luau::BoundTypePack>(&current->ty)) {
                    current = bound->boundTo;
                } else if (const auto *sequence = Luau::get<Luau::TypePack>(current)) {
                    for (const auto type : sequence->head) {
                        budget.check();
                        head.push_back(type);
                    }

                    current = sequence->tail.value_or(nullptr);
                } else {
                    break;
                }
            }

            return {std::move(head), current ? std::optional<Luau::TypePackId>(current) : std::nullopt};
        }

        Luau::TypeId call_type(const Luau::Module &module, Luau::AstExprCall *call) {
            if (const auto *type = module.astOriginalCallTypes.find(call)) {
                return *type;
            }

            if (const auto *type = module.astTypes.find(call->func)) {
                return *type;
            }

            return nullptr;
        }

        bool is_call_complete(const Prepared &prepared, const Luau::AstExprCall &call) {
            const auto begin = offset(prepared.text, call.argLocation.begin);
            const auto end = offset(prepared.text, call.argLocation.end);

            if (begin == 0 || prepared.text[begin - 1] != '(') {
                return false;
            }

            Luau::Allocator allocator;
            Luau::AstNameTable names(allocator);
            Luau::Lexer lexer(prepared.text.data() + begin, end - begin, names);
            lexer.setSkipComments(true);
            unsigned depth = 0;

            for (auto token = lexer.next(); token.type != Luau::Lexeme::Eof; token = lexer.next()) {
                if (token.type == '(') {
                    ++depth;
                } else if (token.type == ')') {
                    if (depth == 0) {
                        return true;
                    }

                    --depth;
                }
            }

            return false;
        }

        std::optional<Luau::AutocompleteEntryMap> strings(Luau::ScopePtr scope, const std::string &tag) {
            if (tag != "instar.service" && tag != "instar.creatable" && tag != "instar.class") {
                return std::nullopt;
            }

            Luau::AutocompleteEntryMap result;

            while (scope) {
                for (const auto &[name, binding] : scope->exportedTypeBindings) {
                    const auto type = Luau::follow(binding.type, Luau::FollowOption::DisableLazyTypeThunks);
                    const auto *external = Luau::get<Luau::ExternType>(type);

                    if (external &&
                        std::find(external->tags.begin(), external->tags.end(), tag) != external->tags.end()) {
                        Luau::AutocompleteEntry entry;
                        entry.kind = Luau::AutocompleteEntryKind::String;
                        entry.type = type;
                        entry.documentationSymbol = type->documentationSymbol;
                        result.emplace(external->name, std::move(entry));
                    }
                }

                scope = scope->parent;
            }

            return result;
        }

        CompletionKind completion_kind(Luau::AutocompleteEntryKind kind) {
            switch (kind) {
            case Luau::AutocompleteEntryKind::Property:
                return CompletionKind::Property;

            case Luau::AutocompleteEntryKind::Binding:
                return CompletionKind::Binding;

            case Luau::AutocompleteEntryKind::Keyword:
                return CompletionKind::Keyword;

            case Luau::AutocompleteEntryKind::String:
                return CompletionKind::String;

            case Luau::AutocompleteEntryKind::Type:
                return CompletionKind::Type;

            case Luau::AutocompleteEntryKind::Module:
                return CompletionKind::Module;

            case Luau::AutocompleteEntryKind::GeneratedFunction:
                return CompletionKind::GeneratedFunction;

            case Luau::AutocompleteEntryKind::RequirePath:
                return CompletionKind::RequirePath;

            case Luau::AutocompleteEntryKind::HotComment:
                return CompletionKind::HotComment;
            }

            throw std::runtime_error("Unknown native completion kind.");
        }

        class Collision final : public Luau::AstVisitor {
        public:
            explicit Collision(const std::string &name) : name(name) {}

            bool visit(Luau::AstType *) override {
                return true;
            }

            bool visit(Luau::AstTypePack *) override {
                return true;
            }

            bool visit(Luau::AstTypeReference *node) override {
                has_collision = has_collision || node->name.value == name;

                return true;
            }

            bool visit(Luau::AstTypePackGeneric *node) override {
                has_collision = has_collision || node->genericName.value == name;

                return true;
            }

            bool visit(Luau::AstStatTypeFunction *node) override {
                const bool was_in_type_function = is_in_type_function;
                is_in_type_function = true;
                node->body->visit(this);
                is_in_type_function = was_in_type_function;

                return false;
            }

            bool visit(Luau::AstExprGlobal *node) override {
                has_collision = has_collision || (is_in_type_function && node->name.value == name);

                return true;
            }

            bool has_collision = false;

        private:
            const std::string &name;
            bool is_in_type_function = false;
        };

        bool has_type_collision(const Luau::SourceModule &source, const Index &indexed, const std::string &name) {
            if (std::any_of(indexed.entries.begin(), indexed.entries.end(), [&](const Entry &entry) {
                    return entry.is_type_namespace && entry.name == name;
                })) {
                return true;
            }

            Collision visitor(name);
            source.root->visit(&visitor);

            return visitor.has_collision;
        }

        class Hints final : public Luau::AstVisitor {
        public:
            Hints(
                const Host &host, const Luau::Module &module, const Luau::SourceModule &source,
                PresentationBudget &budget, bool is_expanded, Luau::Position start, Luau::Position end,
                std::optional<HintKind> selected = std::nullopt
            )
                : host(host), module(module), source(source), budget(budget), is_expanded(is_expanded), start(start),
                  end(end), selected(selected) {}

            bool visit(Luau::AstNode *node) override {
                budget.check();

                return !(node->location.end < start || end < node->location.begin);
            }

            bool visit(Luau::AstType *node) override {
                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstTypePack *node) override {
                return visit(static_cast<Luau::AstNode *>(node));
            }

            bool visit(Luau::AstStatLocal *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (std::size_t index = 0; index < node->vars.size; ++index) {
                    const auto *value = index < node->values.size ? node->values.data[index] : nullptr;
                    const auto *initializer = value;

                    while (const auto *group = initializer ? initializer->as<Luau::AstExprGroup>() : nullptr) {
                        initializer = group->expr;
                    }

                    const auto *call = initializer ? initializer->as<Luau::AstExprCall>() : nullptr;
                    const auto *global = call ? call->func->as<Luau::AstExprGlobal>() : nullptr;

                    if (global && global->name == "require") {
                        continue;
                    }

                    const bool is_obvious =
                        value &&
                        (value->is<Luau::AstExprConstantNil>() || value->is<Luau::AstExprConstantBool>() ||
                            value->is<Luau::AstExprConstantNumber>() || value->is<Luau::AstExprConstantInteger>() ||
                            value->is<Luau::AstExprConstantString>());

                    binding(node->vars.data[index], is_obvious);
                }

                return true;
            }

            bool visit(Luau::AstStatFor *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                binding(node->var);

                return true;
            }

            bool visit(Luau::AstStatForIn *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (auto *local : node->vars) {
                    binding(local);
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                for (auto *local : node->args) {
                    binding(local);
                }

                if (!node->returnAnnotation && node->argLocation) {
                    if (const auto *type = module.astTypes.find(node)) {
                        if (const auto *function = Luau::get<Luau::FunctionType>(
                                Luau::follow(*type, Luau::FollowOption::DisableLazyTypeThunks)
                            )) {
                            add(
                                node->argLocation->end,
                                {},
                                nullptr,
                                function->retTypes,
                                HintKind::Return,
                                false,
                                documentation(host, *type, documentation(source, module, node->location.begin, budget))
                            );
                        }
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprCall *node) override {
                if (!visit(static_cast<Luau::AstNode *>(node))) {
                    return false;
                }

                const Luau::TypeId *resolved = module.astOverloadResolvedTypes.find(node);
                const auto types = overloads(resolved ? *resolved : call_type(module, node), budget);

                if (types.size() == 1) {
                    const auto *function = Luau::get<Luau::FunctionType>(types.front());
                    const std::size_t skip = node->self ? 1 : 0;
                    const auto [arguments, tail] = parameter_types(function->argTypes, budget);
                    Printer printer(budget, Luau::findScopeAtPosition(module, node->location.begin));

                    const auto reference = documentation(
                        host,
                        types.front(),
                        documentation(source, module, node->func, budget),
                        printer.signature(types.front())
                    );

                    for (std::size_t index = 0; index < node->args.size && index + skip < function->argNames.size();
                        ++index) {
                        budget.check();

                        if (const auto &name = function->argNames[index + skip]) {
                            const auto argument = Luau::getIdentifier(node->args.data[index]);

                            const auto parameter = index + skip < arguments.size() ? arguments[index + skip] : nullptr;

                            add(node->args.data[index]->location.begin,
                                name->name,
                                parameter,
                                parameter ? nullptr : tail.value_or(nullptr),
                                HintKind::Parameter,
                                argument.value && argument.value == name->name,
                                reference,
                                static_cast<std::uint32_t>(index + skip));
                        }
                    }
                }

                return true;
            }

            rust::Vec<NativeHint> result;

        private:
            void binding(Luau::AstLocal *local, bool is_obvious = false) {
                budget.check();

                if (!local->annotation) {
                    if (const auto type = local_type(module, local)) {
                        rust::Vec<SourceLocation> declarations;
                        declarations.push_back(locate(host, module.name, local->location));

                        add(local->location.end,
                            {},
                            type,
                            nullptr,
                            HintKind::Type,
                            is_obvious,
                            documentation(
                                host,
                                type,
                                documentation(source, module, local->location.begin, budget),
                                {},
                                std::move(declarations)
                            ));
                    }
                }
            }

            void
            add(Luau::Position position, const std::string &name, Luau::TypeId type, Luau::TypePackId pack,
                HintKind kind, bool is_obvious, const Documentation &reference,
                std::optional<std::uint32_t> parameter_index = std::nullopt) {
                budget.check();

                if (position < start || end < position || (selected && (*selected != kind || position != start))) {
                    return;
                }

                Printer printer(budget, Luau::findScopeAtPosition(module, position));

                auto presentation =
                    pack ? printer.presentation(pack, is_expanded) : printer.presentation(type, is_expanded);

                if (kind != HintKind::Parameter) {
                    printer.abbreviate(presentation);
                }

                result.push_back(
                    {locate(host, module.name, {position, position}),
                        name,
                        std::move(presentation),
                        kind,
                        is_obvious,
                        reference,
                        bool(parameter_index),
                        parameter_index.value_or(0)}
                );
            }

            const Host &host;
            const Luau::Module &module;
            const Luau::SourceModule &source;
            PresentationBudget &budget;
            bool is_expanded;
            Luau::Position start;
            Luau::Position end;
            std::optional<HintKind> selected;
        };
    }

    std::vector<Luau::TypeId> overloads(Luau::TypeId root, PresentationBudget &budget) {
        std::vector<Luau::TypeId> result;
        std::vector<Luau::TypeId> pending;
        std::unordered_set<Luau::TypeId> seen;

        if (root) {
            pending.push_back(root);
        }

        while (!pending.empty()) {
            budget.check();
            const auto type = pending.back();
            pending.pop_back();

            if (!seen.insert(type).second) {
                continue;
            }

            if (const auto *bound = Luau::get_if<Luau::BoundType>(&type->ty)) {
                pending.push_back(bound->boundTo);
            } else if (Luau::get<Luau::FunctionType>(type)) {
                result.push_back(type);
            } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (auto iterator = intersection->parts.rbegin(); iterator != intersection->parts.rend(); ++iterator) {
                    budget.check();
                    pending.push_back(*iterator);
                }
            } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                for (auto iterator = union_type->options.rbegin(); iterator != union_type->options.rend(); ++iterator) {
                    budget.check();
                    pending.push_back(*iterator);
                }
            } else if (const auto *table = Luau::get<Luau::TableType>(type); table && table->boundTo) {
                pending.push_back(*table->boundTo);
            } else if (const auto *lazy = Luau::get<Luau::LazyType>(type)) {
                if (const auto value = lazy->unwrapped.load()) {
                    pending.push_back(value);
                }
            }
        }

        return result;
    }

    rust::Vec<NativeCompletion> completion(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget, std::vector<CompletionType> &types, std::size_t &candidate_index,
        std::optional<std::size_t> selected
    ) {
        rust::Vec<NativeCompletion> result;
        const auto &name = prepared.module.name;

        auto replace = replacement(prepared.text, position);
        const auto scope = Luau::findScopeAtPosition(prepared.module, position);

        const auto native = Luau::autocomplete(
            frontend,
            name,
            position,
            [&](const std::string &tag, std::optional<const Luau::ExternType *>, const std::optional<std::string> &) {
                return strings(scope, tag);
            }
        );

        Luau::Location operation{position, position};
        Luau::TypeId owner = nullptr;
        std::optional<Luau::AstName> type_namespace;

        for (const auto *node : native.ancestry) {
            if (const auto *index = node->as<Luau::AstExprIndexName>()) {
                if (index->indexLocation.containsClosed(position)) {
                    replace = index->indexLocation;
                    operation = {index->opPosition, 1};

                    if (const auto *type = prepared.module.astTypes.find(index->expr)) {
                        owner = *type;
                    }
                }
            }

            if (const auto *literal = node->as<Luau::AstExprConstantString>()) {
                if (literal->location.containsClosed(position)) {
                    replace = contents(prepared.text, literal->location);
                }
            }

            if (const auto *reference = node->as<Luau::AstTypeReference>()) {
                type_namespace = reference->prefix;
            }
        }

        std::vector<std::string> labels;

        for (const auto &[label, entry] : native.entryMap) {
            budget.check();
            labels.push_back(label);
        }

        std::sort(labels.begin(), labels.end());

        for (const auto &label : labels) {
            budget.check();
            const auto index = candidate_index++;

            if (selected && index != *selected) {
                continue;
            }

            const auto &entry = native.entryMap.at(label);
            auto candidate_replacement = replace;

            if (entry.kind == Luau::AutocompleteEntryKind::GeneratedFunction) {
                for (const auto *node : native.ancestry) {
                    if (const auto *function = node->as<Luau::AstExprFunction>()) {
                        if (function->argLocation) {
                            candidate_replacement = *function->argLocation;
                            ++candidate_replacement.begin.column;
                            --candidate_replacement.end.column;
                        } else {
                            candidate_replacement = {function->location.begin, position};
                        }
                    }
                }
            }

            const auto parentheses =
                entry.insertText || entry.parens == Luau::ParenthesesRecommendation::None ? Parentheses::None
                : entry.parens == Luau::ParenthesesRecommendation::CursorAfter            ? Parentheses::CursorAfter
                                                                                          : Parentheses::CursorInside;

            const auto type =
                entry.type ? Luau::follow(*entry.type, Luau::FollowOption::DisableLazyTypeThunks) : nullptr;

            bool has_receiver = true;
            const bool is_function = type && is_callable(type, budget, has_receiver);
            rust::Vec<rust::String> presentations;
            bool is_complete = false;
            std::size_t equivalence = 0;

            if (entry.type) {
                Printer printer(budget, scope);
                auto presentation = printer.type(*entry.type);
                equivalence = printer.equivalence(*entry.type, presentation, types);
                presentations.push_back(presentation);
                is_complete = printer.is_complete;
            }

            auto symbol = entry.documentationSymbol.value_or("");
            rust::Vec<SourceLocation> declarations;

            if (const auto declaration = property_declaration(owner, label)) {
                declarations.push_back(locate(host, declaration->identity, declaration->location));
            }

            if (entry.kind == Luau::AutocompleteEntryKind::Type && scope) {
                if (const auto declaration =
                        alias_declaration(frontend, prepared.module, position, label, type_namespace)) {
                    declarations.push_back(locate(host, declaration->identity, declaration->location));
                }
            }

            if (entry.kind == Luau::AutocompleteEntryKind::Binding && scope) {
                if (const auto binding = scope->linearSearchForBindingPair(label, true);
                    binding && binding->first.local) {
                    declarations.push_back(locate(host, name, binding->first.local->location));

                    if (symbol.empty()) {
                        symbol = documentation(
                            prepared.source,
                            prepared.module,
                            binding->first.local->location.begin,
                            budget
                        );
                    }
                }
            }

            result.push_back(
                {label,
                    completion_kind(entry.kind),
                    is_function,
                    is_function && has_receiver,
                    std::move(presentations),
                    is_complete,
                    equivalence,
                    documentation(host, type, symbol, {}, std::move(declarations)),
                    entry.insertText.value_or(label),
                    parentheses,
                    locate(host, name, candidate_replacement),
                    entry.replaceDotWithColon,
                    range(operation),
                    entry.deprecated}
            );
        }

        return result;
    }

    rust::Vec<NativeSignature> signatures(
        Luau::Frontend &, const Host &host, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget
    ) {
        constexpr std::size_t maximum_length = 32768;
        rust::Vec<NativeSignature> result;
        const auto *checked = &prepared.module;
        const auto &source = prepared.source;
        const auto &name = checked->name;

        const auto cursor = offset(prepared.text, position);
        const auto ancestry = Luau::findAncestryAtPositionForAutocomplete(source, position);
        Luau::AstExprCall *call = nullptr;

        for (auto *node : ancestry) {
            if (auto *candidate = node->as<Luau::AstExprCall>()) {
                if (!candidate->func->location.contains(position) && candidate->argLocation.containsClosed(position) &&
                    (position != candidate->argLocation.end || !is_call_complete(prepared, *candidate))) {
                    call = candidate;
                }
            }
        }

        if (!call) {
            return result;
        }

        unsigned active = 0;

        for (std::size_t index = 0; index < call->args.size; ++index) {
            active = static_cast<unsigned>(index);

            if (position <= call->args.data[index]->location.end) {
                break;
            }

            if (index + 1 == call->args.size) {
                const auto end = offset(prepared.text, call->args.data[index]->location.end);

                if (end < cursor) {
                    Luau::Allocator allocator;
                    Luau::AstNameTable names(allocator);
                    Luau::Lexer lexer(prepared.text.data() + end, cursor - end, names);
                    lexer.setSkipComments(true);

                    if (lexer.next().type == ',') {
                        ++active;
                    }
                }
            }
        }

        const auto *resolved = checked->astOverloadResolvedTypes.find(call);

        for (const auto type : overloads(call_type(*checked, call), budget)) {
            const auto *function = Luau::get<Luau::FunctionType>(type);
            budget.check();

            Printer printer(budget, Luau::findScopeAtPosition(*checked, position));
            std::string header = function->isCheckedFunction ? "@checked " : "";
            bool is_first_generic = true;

            const auto generic = [&](const std::string &text) {
                header += is_first_generic ? "<" : ", ";
                header += text;
                is_first_generic = false;
            };

            for (const auto type : function->generics) {
                generic(printer.type(type));
            }

            for (const auto pack : function->genericPacks) {
                generic(printer.pack(pack));
            }

            if (!is_first_generic) {
                header += '>';
            }

            std::string label = header + "(";
            std::string expanded = label;
            rust::Vec<NativeParameter> parameters;
            const auto [arguments, tail] = parameter_types(function->argTypes, budget);
            const std::size_t skip = call->self && !arguments.empty() ? 1 : 0;

            const auto parameter =
                [&](std::size_t index, const std::string &name, const NativePresentation &text, bool is_variadic) {
                    budget.check();

                    if (!parameters.empty()) {
                        label += ", ";
                        expanded += ", ";
                    }

                    const auto start = label.size();
                    const auto prefix = !is_variadic && !name.empty() ? name + ": " : "";
                    label += prefix + std::string(text.compact);
                    expanded += prefix + std::string(text.expanded);

                    if (label.size() > maximum_length || expanded.size() > maximum_length) {
                        throw std::runtime_error("Signature exceeds presentation limits.");
                    }

                    parameters.push_back({name, start, label.size(), static_cast<std::uint32_t>(index), is_variadic});
                };

            for (std::size_t index = skip; index < arguments.size(); ++index) {
                std::string argument_name;

                if (index < function->argNames.size()) {
                    if (const auto &name = function->argNames[index]) {
                        argument_name = name->name;
                    }
                }

                parameter(index, argument_name, printer.presentation(arguments[index], true), false);
            }

            if (tail) {
                const auto *variadic = Luau::get_if<Luau::VariadicTypePack>(&(*tail)->ty);

                if (!variadic || !variadic->hidden) {
                    parameter(arguments.size(), "...", printer.presentation(*tail, true), true);
                }
            }

            label += ") -> " + printer.pack(function->retTypes, false, true);
            expanded += ") -> " + printer.pack(function->retTypes, true, true);

            if (label.size() > maximum_length) {
                throw std::runtime_error("Signature exceeds presentation limits.");
            }

            result.push_back(
                {label,
                    printer.finish(std::move(expanded)),
                    documentation(
                        host,
                        type,
                        documentation(source, *checked, call->func, budget),
                        printer.signature(type)
                    ),
                    std::move(parameters),
                    active,
                    resolved && Luau::follow(*resolved, Luau::FollowOption::DisableLazyTypeThunks) == type}
            );
        }

        return result;
    }

    rust::Vec<SourceLocation> definitions(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        bool should_resolve_types
    ) {
        rust::Vec<SourceLocation> result;
        const auto *checked = &prepared.module;
        const auto &source = prepared.source;
        const auto &name = checked->name;

        offset(prepared.text, position);
        const auto indexed = index(frontend, *checked, source);

        if (const auto *entry = at(indexed, position)) {
            if (!should_resolve_types || entry->is_type_namespace) {
                result.push_back(locate(host, entry->declaration.identity, entry->declaration.location));

                return result;
            }
        }

        if (!should_resolve_types) {
            const auto trace = frontend.requireTrace.find(name);
            bool has_import = false;

            if (trace != frontend.requireTrace.end()) {
                for (const auto &[node, information] : trace->second.exprs) {
                    const auto *call = node->as<Luau::AstExprCall>();
                    const auto *global = call ? call->func->as<Luau::AstExprGlobal>() : nullptr;

                    if (call && global && global->name == "require" && call->args.size && !information.name.empty() &&
                        call->args.data[0]->location.contains(position)) {
                        result.push_back(locate(host, information.name, {{0, 0}, {0, 0}}));
                        has_import = true;
                    }
                }
            }

            if (has_import) {
                return result;
            }
        }

        auto expression = Luau::findExprOrLocalAtPosition(source, position);
        Luau::TypeId type = nullptr;

        if (auto *local = expression.getLocal()) {
            type = local_type(*checked, local);

            if (should_resolve_types) {
                if (const auto target = annotation_declaration(frontend, *checked, local->annotation)) {
                    result.push_back(locate(host, target->identity, target->location));

                    return result;
                }
            }
        } else if (const auto *node = expression.getExpr()) {
            if (should_resolve_types) {
                if (const auto *reference = node->as<Luau::AstExprLocal>()) {
                    if (const auto target = annotation_declaration(frontend, *checked, reference->local->annotation)) {
                        result.push_back(locate(host, target->identity, target->location));

                        return result;
                    }
                }
            }

            if (const auto *found = checked->astTypes.find(node)) {
                type = *found;
            }
        }

        const auto ancestry = Luau::findAstAncestryOfPosition(source, position, true);

        if (!ancestry.empty()) {
            if (const auto *annotation = ancestry.back()->asType()) {
                if (const auto *found = checked->astResolvedTypes.find(annotation)) {
                    type = *found;
                }
            }
        }

        if (const auto target = definition(type)) {
            result.push_back(locate(host, target->identity, target->location));
        }

        return result;
    }

    Entry target(Luau::Frontend &frontend, const Prepared &prepared, Luau::Position position) {
        offset(prepared.text, position);
        const auto indexed = index(frontend, prepared.module, prepared.source);
        const auto *entry = at(indexed, position);

        if (!entry) {
            throw std::invalid_argument("Rename target has no semantic declaration.");
        }

        if (!entry->is_editable) {
            throw std::invalid_argument("Rename is unavailable for this source representation.");
        }

        return *entry;
    }

    rust::Vec<Occurrence> rename(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, const std::string &name,
        const Entry &target, bool &declared
    ) {
        const auto &module = prepared.module;
        const auto &source = prepared.source;

        if (!source.parseErrors.empty() || module.constraintGenerationDidNotComplete) {
            throw std::invalid_argument(
                "Rename validation requires complete candidate syntax and binding information."
            );
        }

        const auto indexed = index(frontend, module, source);
        const auto kind = target.kind;
        const bool is_type_namespace = target.is_type_namespace;

        const bool is_affected = host.getHumanReadableModuleName(target.declaration.identity) ==
                                     host.getHumanReadableModuleName(module.name) ||
                                 std::any_of(indexed.entries.begin(), indexed.entries.end(), [&](const Entry &entry) {
                                     return is_same_declaration(host, target.declaration, entry.declaration) &&
                                            target.is_type_namespace == entry.is_type_namespace;
                                 });

        if ((is_affected || kind == BindingKind::Property) && name != target.name) {
            if (kind == BindingKind::Property) {
                if (indexed.properties.count(name)) {
                    throw std::invalid_argument("Rename would conflict with an existing property.");
                }
            } else {
                if (indexed.names.count(name) && (!is_type_namespace || has_type_collision(source, indexed, name))) {
                    throw std::invalid_argument("Rename would conflict with an existing value or type binding.");
                }

                for (const auto &[location, scope] : module.scopes) {
                    if ((!is_type_namespace && scope->linearSearchForBinding(name)) || scope->lookupType(name) ||
                        scope->lookupPack(name)) {
                        throw std::invalid_argument("Rename would capture an existing value or type binding.");
                    }
                }
            }
        }

        rust::Vec<Occurrence> result;
        std::vector<std::pair<std::size_t, std::size_t>> edits;

        for (const auto &entry : indexed.entries) {
            if (!is_same_declaration(host, target.declaration, entry.declaration) ||
                target.is_type_namespace != entry.is_type_namespace) {
                continue;
            }

            if (!entry.is_editable) {
                throw std::invalid_argument("Rename is unavailable for the source representation of an occurrence.");
            }

            if (entry.is_type_namespace && name != target.name) {
                for (const auto *node : Luau::findAstAncestryOfPosition(source, entry.location.begin, true)) {
                    if (const auto *global = node->as<Luau::AstExprGlobal>();
                        global && global->location == entry.location) {
                        const auto scope = Luau::findScopeAtPosition(module, entry.location.begin);

                        if (scope && scope->linearSearchForBinding(name)) {
                            throw std::invalid_argument(
                                "Rename would capture an existing type-function value binding."
                            );
                        }
                    }
                }
            }

            if (name == "typeof" && entry.role == OccurrenceRole::Declaration && entry.is_type_namespace) {
                for (const auto *node : Luau::findAstAncestryOfPosition(source, entry.location.begin, true)) {
                    const auto *alias = node->as<Luau::AstStatTypeAlias>();
                    const auto *function = node->as<Luau::AstStatTypeFunction>();

                    if ((alias && alias->nameLocation == entry.location) ||
                        (function && function->nameLocation == entry.location)) {
                        throw std::invalid_argument("Type declarations cannot be named `typeof`.");
                    }
                }
            }

            declared = declared || entry.role == OccurrenceRole::Declaration;
            result.push_back(occurrence(host, module.name, entry));

            edits.emplace_back(offset(prepared.text, entry.location.begin), offset(prepared.text, entry.location.end));
        }

        if (!edits.empty()) {
            std::sort(edits.begin(), edits.end(), std::greater<>());
            edits.erase(std::unique(edits.begin(), edits.end()), edits.end());
            auto edited = prepared.text;

            for (const auto &[begin, end] : edits) {
                edited.replace(begin, end - begin, name);
            }

            Luau::Allocator allocator;
            Luau::AstNameTable names(allocator);

            const auto parsed = Luau::Parser::parse(
                edited.data(),
                edited.size(),
                names,
                allocator,
                host.getConfig(module.name, {}).parseOptions
            );

            if (!parsed.root || !parsed.errors.empty()) {
                throw std::invalid_argument("Rename produces invalid syntax in a source context.");
            }
        }

        return result;
    }

    rust::Vec<NativeHint> hints(
        Luau::Frontend &, const Host &host, const Luau::Module &module, const Luau::SourceModule &source,
        PresentationBudget &budget, bool is_expanded, Luau::Position start, Luau::Position end,
        std::optional<HintKind> selected
    ) {
        Hints visitor(host, module, source, budget, is_expanded, start, end, selected);
        source.root->visit(&visitor);

        return std::move(visitor.result);
    }

    rust::Vec<Import> imports(const Host &host, const std::string &name, const Luau::RequireTraceResult &trace) {
        rust::Vec<Import> result;

        for (const auto &[node, information] : trace.exprs) {
            const auto *call = node->as<Luau::AstExprCall>();
            const auto *global = call ? call->func->as<Luau::AstExprGlobal>() : nullptr;

            if (call && global && global->name == "require" && call->args.size && !information.name.empty()) {
                result.push_back(
                    {locate(host, name, call->args.data[0]->location),
                        host.getHumanReadableModuleName(information.name),
                        information.name,
                        host.is_instance(information.name)}
                );
            }
        }

        return result;
    }

    SourceLocation locate(const Host &host, const std::string &identity, const Luau::Location &location) {
        return {host.getHumanReadableModuleName(identity), identity, range(location)};
    }

    std::size_t offset(const std::string &source, Luau::Position position) {
        std::size_t start = 0;

        for (unsigned line = 0; line < position.line; ++line) {
            const auto newline = source.find('\n', start);

            if (newline == std::string::npos) {
                throw std::invalid_argument("Semantic query line is outside the source.");
            }

            start = newline + 1;
        }

        const auto newline = source.find('\n', start);
        const auto end = newline == std::string::npos ? source.size() : newline;

        if (position.column > end - start) {
            throw std::invalid_argument("Semantic query column is outside the source line.");
        }

        return start + position.column;
    }

    Documentation documentation(
        const Host &host, Luau::TypeId type, const std::string &symbol, const std::string &signature,
        rust::Vec<SourceLocation> declarations
    ) {
        if (const auto declaration = definition(type)) {
            const auto location = locate(host, declaration->identity, declaration->location);

            const bool is_present =
                std::any_of(declarations.begin(), declarations.end(), [&](const SourceLocation &candidate) {
                    return candidate.identity == location.identity &&
                           candidate.range.start.line == location.range.start.line &&
                           candidate.range.start.column == location.range.start.column &&
                           candidate.range.end.line == location.range.end.line &&
                           candidate.range.end.column == location.range.end.column;
                });

            if (!is_present) {
                declarations.push_back(location);
            }
        }

        return {symbol, signature, std::move(declarations)};
    }

    std::string documentation(Luau::TypeId root, PresentationBudget &budget) {
        if (!root) {
            return {};
        }

        if (root->documentationSymbol && !root->documentationSymbol->empty()) {
            return *root->documentationSymbol;
        }

        std::vector<Luau::TypeId> pending{root};
        std::unordered_set<Luau::TypeId> visited;
        std::string result;

        while (!pending.empty()) {
            budget.check();
            const auto current = pending.back();
            pending.pop_back();

            if (!visited.insert(current).second) {
                continue;
            }

            const auto type = Luau::follow(current, Luau::FollowOption::DisableLazyTypeThunks);

            if (const auto *primitive = Luau::get<Luau::PrimitiveType>(type);
                primitive && primitive->type == Luau::PrimitiveType::NilType) {
                continue;
            }

            const auto &symbol =
                current->documentationSymbol ? current->documentationSymbol : type->documentationSymbol;

            if (symbol && !symbol->empty()) {
                if (!result.empty() && result != *symbol) {
                    return {};
                }

                result = *symbol;
            } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                pending.insert(pending.end(), union_type->options.begin(), union_type->options.end());
            } else {
                return {};
            }
        }

        return result;
    }

    std::string documentation(
        const Luau::SourceModule &source, const Luau::Module &module, Luau::Position position,
        PresentationBudget &budget
    ) {
        std::unordered_set<const Luau::AstLocal *> visited;

        while (true) {
            budget.check();

            if (const auto symbol = Luau::getDocumentationSymbolAtPosition(source, module, position);
                symbol && !symbol->empty()) {
                return *symbol;
            }

            auto subject = Luau::findExprOrLocalAtPosition(source, position);
            auto *local = subject.getLocal();

            if (const auto *expression = subject.getExpr()) {
                if (const auto *binding = expression->as<Luau::AstExprLocal>()) {
                    local = binding->local;
                }
            }

            if (!local || !visited.insert(local).second) {
                return {};
            }

            const auto type = local_type(module, local);

            if (const auto symbol = documentation(type, budget); !symbol.empty()) {
                return symbol;
            }

            Luau::AstExpr *initializer = nullptr;

            for (const auto *node : Luau::findAstAncestryOfPosition(source, local->location.begin)) {
                budget.check();

                if (const auto *declaration = node->as<Luau::AstStatLocal>()) {
                    for (std::size_t index = 0; index < declaration->vars.size && index < declaration->values.size;
                        ++index) {
                        if (declaration->vars.data[index] == local) {
                            initializer = declaration->values.data[index];
                        }
                    }
                }
            }

            while (const auto *group = initializer ? initializer->as<Luau::AstExprGroup>() : nullptr) {
                initializer = group->expr;
            }

            const auto *inferred = initializer ? module.astTypes.find(initializer) : nullptr;

            if (!inferred || !type ||
                Luau::follow(*inferred, Luau::FollowOption::DisableLazyTypeThunks) !=
                    Luau::follow(type, Luau::FollowOption::DisableLazyTypeThunks)) {
                return {};
            }

            if (const auto *member = initializer->as<Luau::AstExprIndexName>()) {
                position = member->indexLocation.begin;
            } else if (initializer->is<Luau::AstExprIndexExpr>()) {
                return documentation(source, module, initializer, budget);
            } else if (initializer->is<Luau::AstExprLocal>() || initializer->is<Luau::AstExprGlobal>()) {
                position = initializer->location.begin;
            } else {
                return {};
            }
        }
    }

    std::string documentation(
        const Luau::SourceModule &source, const Luau::Module &module, const Luau::AstExpr *expression,
        PresentationBudget &budget
    ) {
        budget.check();

        while (const auto *group = expression->as<Luau::AstExprGroup>()) {
            expression = group->expr;
        }

        if (const auto *member = expression->as<Luau::AstExprIndexName>()) {
            return documentation(source, module, member->indexLocation.begin, budget);
        }

        if (const auto *member = expression->as<Luau::AstExprIndexExpr>()) {
            const auto *key = member->index->as<Luau::AstExprConstantString>();
            const auto *receiver = module.astTypes.find(member->expr);

            if (!key || !receiver) {
                return {};
            }

            const auto owner = Luau::follow(*receiver, Luau::FollowOption::DisableLazyTypeThunks);
            const std::string name(key->value.data, key->value.size);
            const Luau::Property *property = nullptr;

            if (const auto *external = Luau::get<Luau::ExternType>(owner)) {
                property = Luau::lookupExternTypeProp(external, name);
            } else if (const auto *table = Luau::get<Luau::TableType>(owner)) {
                const auto found = table->props.find(name);
                property = found != table->props.end() ? &found->second : nullptr;
            }

            return property ? property->documentationSymbol.value_or("") : std::string{};
        }

        return documentation(source, module, expression->location.begin, budget);
    }

    Occurrence occurrence(const Host &host, const std::string &identity, const Entry &entry) {
        return {locate(host, identity, entry.location),
            locate(host, entry.declaration.identity, entry.declaration.location),
            entry.role,
            entry.is_type_namespace};
    }

    void validate_identifier(const std::string &name) {
        if (name.empty() || (name.front() >= '0' && name.front() <= '9') || Luau::Lexer::isReserved(name) ||
            !Luau::isIdentifier(name)) {
            throw std::invalid_argument("Rename requires a non-keyword Luau identifier.");
        }
    }
}
