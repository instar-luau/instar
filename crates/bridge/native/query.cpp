#include "query.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/Lexer.h"
#include "Luau/Parser.h"
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
                    for (auto iterator = intersection->parts.rbegin(); iterator != intersection->parts.rend();
                        ++iterator) {
                        budget.check();
                        pending.push_back(*iterator);
                    }
                } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                    for (auto iterator = union_type->options.rbegin(); iterator != union_type->options.rend();
                        ++iterator) {
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

        bool is_callable(Luau::TypeId root, PresentationBudget &budget) {
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
                } else if (!Luau::get<Luau::FunctionType>(type)) {
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
                Luau::Frontend &frontend, const Host &host, const Luau::Module &module,
                const Luau::SourceModule &source, PresentationBudget &budget, bool is_expanded, Luau::Position start,
                Luau::Position end, std::optional<HintKind> selected = std::nullopt
            )
                : frontend(frontend), host(host), module(module), source(source), budget(budget),
                  is_expanded(is_expanded), start(start), end(end), selected(selected) {}

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
                            add(node->argLocation->end,
                                {},
                                nullptr,
                                function->retTypes,
                                HintKind::Return,
                                false,
                                documentation(source, module, node->location.begin));
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

                    for (std::size_t index = 0; index < node->args.size && index + skip < function->argNames.size();
                        ++index) {
                        budget.check();

                        if (const auto &name = function->argNames[index + skip]) {
                            const auto argument = Luau::getIdentifier(node->args.data[index]);

                            const auto parameter = index + skip < arguments.size() ? arguments[index + skip] : nullptr;
                            auto symbol = documentation(source, module, node->func->location.begin);

                            add(node->args.data[index]->location.begin,
                                name->name,
                                parameter,
                                parameter ? nullptr : tail.value_or(nullptr),
                                HintKind::Parameter,
                                argument.value && argument.value == name->name,
                                symbol,
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
                        add(local->location.end,
                            {},
                            type,
                            nullptr,
                            HintKind::Type,
                            is_obvious,
                            documentation(source, module, local->location.begin));
                    }
                }
            }

            void
            add(Luau::Position position, const std::string &name, Luau::TypeId type, Luau::TypePackId pack,
                HintKind kind, bool is_obvious = false, const std::string &symbol = {},
                std::optional<std::uint32_t> parameter_index = std::nullopt) {
                budget.check();

                if (position < start || end < position || (selected && (*selected != kind || position != start))) {
                    return;
                }

                Extraction extraction(
                    budget,
                    is_expanded,
                    Luau::findScopeAtPosition(module, position),
                    module,
                    source,
                    *frontend.builtinTypes,
                    frontend.moduleResolver
                );

                const auto root = pack ? extraction.pack(pack) : extraction.type(type);

                result.push_back(
                    {locate(host, module.name, {position, position}),
                        name,
                        extraction.finish(root, bool(pack)),
                        kind,
                        is_obvious,
                        symbol,
                        {},
                        bool(parameter_index),
                        parameter_index.value_or(0)}
                );
            }

            Luau::Frontend &frontend;
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

    rust::Vec<NativeCompletion> completion(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget, std::size_t &candidate_index, std::optional<std::size_t> selected
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

        for (const auto *node : native.ancestry) {
            if (const auto *index = node->as<Luau::AstExprIndexName>()) {
                if (index->indexLocation.containsClosed(position)) {
                    replace = index->indexLocation;
                    operation = {index->opPosition, 1};
                }
            }

            if (const auto *literal = node->as<Luau::AstExprConstantString>()) {
                if (literal->location.containsClosed(position)) {
                    replace = contents(prepared.text, literal->location);
                }
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
            std::string insertion = entry.insertText.value_or(label);
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

            bool is_snippet = false;

            if (!entry.insertText && entry.parens != Luau::ParenthesesRecommendation::None) {
                insertion += entry.parens == Luau::ParenthesesRecommendation::CursorAfter ? "()$0" : "($0)";
                is_snippet = true;
            }

            const auto type =
                entry.type ? Luau::follow(*entry.type, Luau::FollowOption::DisableLazyTypeThunks) : nullptr;

            const bool is_function = type && is_callable(type, budget);
            rust::Vec<NativeSnapshot> snapshots;

            if (entry.type) {
                Extraction extraction(
                    budget,
                    bool(selected),
                    scope,
                    prepared.module,
                    prepared.source,
                    *frontend.builtinTypes,
                    frontend.moduleResolver
                );

                const auto root = extraction.type(*entry.type);
                snapshots.push_back(extraction.finish(root));
            }

            result.push_back(
                {label,
                    completion_kind(entry.kind),
                    is_function,
                    std::move(snapshots),
                    entry.documentationSymbol.value_or(""),
                    {},
                    insertion,
                    is_snippet,
                    locate(host, name, candidate_replacement),
                    entry.replaceDotWithColon,
                    range(operation),
                    entry.deprecated}
            );
        }

        return result;
    }

    rust::Vec<NativeSignature> signatures(
        Luau::Frontend &frontend, const Host &, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget
    ) {
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

            Extraction extraction(
                budget,
                true,
                Luau::findScopeAtPosition(*checked, position),
                *checked,
                source,
                *frontend.builtinTypes,
                frontend.moduleResolver
            );

            const auto root = extraction.type(type);
            rust::Vec<NativeParameter> parameters;
            const auto [arguments, tail] = parameter_types(function->argTypes, budget);
            const std::size_t skip = call->self && !arguments.empty() ? 1 : 0;

            for (std::size_t index = skip; index < arguments.size(); ++index) {
                budget.check();

                std::string argument_name;

                if (index < function->argNames.size()) {
                    if (const auto &name = function->argNames[index]) {
                        argument_name = name->name;
                    }
                }

                parameters.push_back(
                    {argument_name,
                        extraction.type(arguments[index]),
                        false,
                        static_cast<std::uint32_t>(index),
                        {},
                        false}
                );
            }

            if (tail) {
                const auto *variadic = Luau::get_if<Luau::VariadicTypePack>(&(*tail)->ty);

                if (!variadic || !variadic->hidden) {
                    parameters.push_back(
                        {"...", extraction.pack(*tail), true, static_cast<std::uint32_t>(arguments.size()), {}, true}
                    );
                }
            }

            result.push_back(
                {name,
                    extraction.finish(root),
                    documentation(source, *checked, call->func->location.begin),
                    {},
                    std::move(parameters),
                    active,
                    resolved && Luau::follow(*resolved, Luau::FollowOption::DisableLazyTypeThunks) == type}
            );
        }

        return result;
    }

    rust::Vec<SourceLocation> definitions(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        bool resolve_types
    ) {
        rust::Vec<SourceLocation> result;
        const auto *checked = &prepared.module;
        const auto &source = prepared.source;
        const auto &name = checked->name;

        offset(prepared.text, position);
        const auto indexed = index(frontend, *checked, source);

        if (const auto *entry = at(indexed, position)) {
            if (!resolve_types || entry->is_type_namespace) {
                result.push_back(locate(host, entry->declaration.identity, entry->declaration.location));

                return result;
            }
        }

        if (!resolve_types) {
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

            if (resolve_types) {
                if (const auto target = annotation_declaration(frontend, *checked, local->annotation)) {
                    result.push_back(locate(host, target->identity, target->location));

                    return result;
                }
            }
        } else if (const auto *node = expression.getExpr()) {
            if (resolve_types) {
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
        const std::vector<Entry> &targets, std::vector<bool> &declared
    ) {
        const auto &module = prepared.module;
        const auto &source = prepared.source;

        if (!source.parseErrors.empty() || module.constraintGenerationDidNotComplete) {
            throw std::invalid_argument(
                "Rename validation requires complete candidate syntax and binding information."
            );
        }

        const auto indexed = index(frontend, module, source);
        const auto kind = targets.front().kind;
        const bool is_type_namespace = targets.front().is_type_namespace;

        const bool is_affected = std::any_of(targets.begin(), targets.end(), [&](const Entry &target) {
            return target.declaration.identity == module.name ||
                   std::any_of(indexed.entries.begin(), indexed.entries.end(), [&](const Entry &entry) {
                       return is_same_declaration(target.declaration, entry.declaration) &&
                              target.is_type_namespace == entry.is_type_namespace;
                   });
        });

        if ((is_affected || kind == BindingKind::Property) && name != targets.front().name) {
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
            for (std::size_t target = 0; target < targets.size(); ++target) {
                if (!is_same_declaration(targets[target].declaration, entry.declaration) ||
                    targets[target].is_type_namespace != entry.is_type_namespace) {
                    continue;
                }

                if (!entry.is_editable) {
                    throw std::invalid_argument(
                        "Rename is unavailable for the source representation of an occurrence."
                    );
                }

                if (entry.is_type_namespace && name != targets.front().name) {
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

                declared[target] = declared[target] || entry.role == OccurrenceRole::Declaration;
                result.push_back(occurrence(host, module.name, entry));

                edits.emplace_back(
                    offset(prepared.text, entry.location.begin),
                    offset(prepared.text, entry.location.end)
                );

                break;
            }
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
        Luau::Frontend &frontend, const Host &host, const Luau::Module &module, const Luau::SourceModule &source,
        PresentationBudget &budget, bool is_expanded, Luau::Position start, Luau::Position end,
        std::optional<HintKind> selected
    ) {
        Hints visitor(frontend, host, module, source, budget, is_expanded, start, end, selected);
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
                        information.name}
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

    std::string documentation(const Luau::SourceModule &source, const Luau::Module &module, Luau::Position position) {
        const auto symbol = Luau::getDocumentationSymbolAtPosition(source, module, position);

        return symbol ? *symbol : std::string{};
    }

    Occurrence occurrence(const Host &host, const std::string &identity, const Entry &entry) {
        return {locate(host, identity, entry.location),
            locate(host, entry.declaration.identity, entry.declaration.location),
            entry.role,
            entry.is_type_namespace};
    }

    void validate_identifier(const std::string &name) {
        if (name.empty() || (name.front() >= '0' && name.front() <= '9') || Luau::Lexer::isReserved(name) ||
            !std::all_of(name.begin(), name.end(), [](unsigned char value) {
                return is_identifier_character(value);
            })) {
            throw std::invalid_argument("Rename requires a non-keyword Luau identifier.");
        }
    }
}
