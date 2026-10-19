#include "query.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/Lexer.h"
#include "Luau/Parser.h"
#include "Luau/ToString.h"
#include "Luau/TypeCheckLimits.h"
#include "Luau/TypePack.h"

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

        SourceLocation locate(const Host &host, const std::string &identity, const Luau::Location &location) {
            return {host.getHumanReadableModuleName(identity), identity, range(location)};
        }

        std::size_t offset(const std::string &source, Luau::Position position) {
            std::size_t start = 0;

            for (unsigned line = 0; line < position.line; ++line) {
                const auto newline = source.find('\n', start);

                if (newline == std::string::npos) {
                    throw std::invalid_argument("semantic query line is outside the source");
                }

                start = newline + 1;
            }

            const auto newline = source.find('\n', start);
            const auto end = newline == std::string::npos ? source.size() : newline;

            if (position.column > end - start) {
                throw std::invalid_argument("semantic query column is outside the source");
            }

            return start + position.column;
        }

        bool character(unsigned char value) {
            return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z') || (value >= '0' && value <= '9') ||
                   value == '_';
        }

        Luau::Location replacement(const std::string &source, Luau::Position position) {
            const auto cursor = offset(source, position);
            auto start = cursor;
            auto end = cursor;

            while (start > 0 && character(static_cast<unsigned char>(source[start - 1]))) {
                --start;
            }

            while (end < source.size() && character(static_cast<unsigned char>(source[end]))) {
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

        void overloads(Luau::TypeId type, std::vector<Luau::TypeId> &result, std::unordered_set<Luau::TypeId> &seen) {
            if (!type) {
                return;
            }

            type = Luau::follow(type);

            if (!seen.insert(type).second) {
                return;
            }

            if (Luau::get<Luau::FunctionType>(type)) {
                result.push_back(type);
            } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (auto part : intersection->parts) {
                    overloads(part, result, seen);
                }
            } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                for (auto part : union_type->options) {
                    overloads(part, result, seen);
                }
            }
        }

        std::vector<Luau::TypeId> overloads(Luau::TypeId type) {
            std::vector<Luau::TypeId> result;
            std::unordered_set<Luau::TypeId> seen;
            overloads(type, result, seen);

            return result;
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

        bool completed(const Prepared &prepared, const Luau::AstExprCall &call) {
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
                    const auto type = Luau::follow(binding.type);
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

        std::string
        documentation(const Luau::SourceModule &source, const Luau::Module &module, Luau::Position position) {
            const auto symbol = Luau::getDocumentationSymbolAtPosition(source, module, position);

            return symbol ? *symbol : std::string{};
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

            throw std::runtime_error("unknown native completion kind");
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
                result = result || node->name.value == name;

                return true;
            }

            bool visit(Luau::AstTypePackGeneric *node) override {
                result = result || node->genericName.value == name;

                return true;
            }

            bool visit(Luau::AstStatTypeFunction *node) override {
                const bool previous = body;
                body = true;
                node->body->visit(this);
                body = previous;

                return false;
            }

            bool visit(Luau::AstExprGlobal *node) override {
                result = result || (body && node->name.value == name);

                return true;
            }

            bool result = false;

        private:
            const std::string &name;
            bool body = false;
        };

        bool type_collision(const Luau::SourceModule &source, const Index &indexed, const std::string &name) {
            if (std::any_of(indexed.entries.begin(), indexed.entries.end(), [&](const Entry &entry) {
                    return entry.type_namespace && entry.name == name;
                })) {
                return true;
            }

            Collision visitor(name);
            source.root->visit(&visitor);

            return visitor.result;
        }

        class Hints final : public Luau::AstVisitor {
        public:
            Hints(const Host &host, const Luau::Module &module) : host(host), module(module) {}

            bool visit(Luau::AstType *) override {
                return true;
            }

            bool visit(Luau::AstTypePack *) override {
                return true;
            }

            bool visit(Luau::AstStatLocal *node) override {
                for (std::size_t index = 0; index < node->vars.size; ++index) {
                    const auto *value = index < node->values.size ? node->values.data[index] : nullptr;

                    const bool obvious =
                        value &&
                        (value->is<Luau::AstExprConstantNil>() || value->is<Luau::AstExprConstantBool>() ||
                            value->is<Luau::AstExprConstantNumber>() || value->is<Luau::AstExprConstantInteger>() ||
                            value->is<Luau::AstExprConstantString>());

                    binding(node->vars.data[index], obvious);
                }

                return true;
            }

            bool visit(Luau::AstStatFor *node) override {
                binding(node->var);

                return true;
            }

            bool visit(Luau::AstStatForIn *node) override {
                for (auto *local : node->vars) {
                    binding(local);
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *node) override {
                for (auto *local : node->args) {
                    binding(local);
                }

                if (!node->returnAnnotation && node->argLocation) {
                    if (const auto *type = module.astTypes.find(node)) {
                        if (const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                            add(node->argLocation->end,
                                ": (" + Luau::toString(function->retTypes) + ")",
                                HintKind::Return);
                        }
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprCall *node) override {
                const Luau::TypeId *resolved = module.astOverloadResolvedTypes.find(node);
                const auto types = overloads(resolved ? *resolved : call_type(module, node));

                if (types.size() == 1) {
                    const auto *function = Luau::get<Luau::FunctionType>(types.front());
                    const std::size_t skip = node->self ? 1 : 0;

                    for (std::size_t index = 0; index < node->args.size && index + skip < function->argNames.size();
                        ++index) {
                        if (const auto &name = function->argNames[index + skip]) {
                            const auto argument = Luau::getIdentifier(node->args.data[index]);

                            add(node->args.data[index]->location.begin,
                                name->name + ":",
                                HintKind::Parameter,
                                argument.value && argument.value == name->name);
                        }
                    }
                }

                return true;
            }

            rust::Vec<Hint> result;

        private:
            void binding(Luau::AstLocal *local, bool obvious = false) {
                if (!local->annotation) {
                    if (const auto type = local_type(module, local)) {
                        add(local->location.end, ": " + Luau::toString(type), HintKind::Type, obvious);
                    }
                }
            }

            void add(Luau::Position position, const std::string &label, HintKind kind, bool obvious = false) {
                result.push_back({locate(host, module.name, {position, position}), label, kind, obvious});
            }

            const Host &host;
            const Luau::Module &module;
        };
    }

    Occurrence occurrence(const Host &host, const std::string &identity, const Entry &entry) {
        return {locate(host, identity, entry.location),
            locate(host, entry.declaration.identity, entry.declaration.location),
            entry.role,
            entry.type_namespace};
    }

    void identifier(const std::string &name) {
        if (name.empty() || (name.front() >= '0' && name.front() <= '9') || Luau::Lexer::isReserved(name) ||
            !std::all_of(name.begin(), name.end(), [](unsigned char value) { return character(value); })) {
            throw std::invalid_argument("rename requires a non-keyword Luau identifier");
        }
    }

    rust::Vec<Hover>
    hover(Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position) {
        rust::Vec<Hover> result;
        const auto *checked = &prepared.module;
        const auto &source = prepared.source;
        const auto &name = checked->name;

        offset(prepared.text, position);
        auto expression = Luau::findExprOrLocalAtPosition(source, position);
        Luau::TypeId type = nullptr;
        Luau::Location location;
        std::string symbol;

        if (auto *local = expression.getLocal()) {
            type = local_type(*checked, local);
            location = local->location;
            symbol = local->name.value;
        } else if (auto *node = expression.getExpr()) {
            if (const auto *found = checked->astTypes.find(node)) {
                type = *found;
            }

            location = node->location;

            if (const auto identifier = expression.getName()) {
                symbol = identifier->value;
            }

            if (const auto *index = node->as<Luau::AstExprIndexName>()) {
                location = index->indexLocation;
                symbol = index->index.value;
            }
        }

        const auto ancestry = Luau::findAstAncestryOfPosition(source, position, true);

        if (!ancestry.empty()) {
            if (auto *annotation = ancestry.back()->asType()) {
                if (const auto *found = checked->astResolvedTypes.find(annotation)) {
                    type = *found;
                    location = annotation->location;

                    if (const auto *reference = annotation->as<Luau::AstTypeReference>()) {
                        symbol = reference->name.value;
                        location = reference->nameLocation;
                    }
                }
            }
        }

        for (auto *node : ancestry) {
            if (const auto *index = node->as<Luau::AstExprIndexExpr>()) {
                if (const auto *key = index->index->as<Luau::AstExprConstantString>()) {
                    if (key->location.contains(position)) {
                        if (const auto *found = checked->astTypes.find(index)) {
                            type = *found;
                            location = contents(prepared.text, key->location);
                            symbol.assign(key->value.data, key->value.size);
                        }
                    }
                }
            }
        }

        if (!type) {
            const auto indexed = index(frontend, *checked, source);

            if (const auto *entry = at(indexed, position)) {
                if (entry->type_namespace) {
                    const auto scope = Luau::findScopeAtPosition(*checked, position);
                    const auto binding = scope ? scope->lookupType(entry->name) : std::nullopt;

                    if (binding) {
                        type = binding->type;
                        location = entry->location;
                        symbol = entry->name;
                    }
                }
            }
        }

        if (type) {
            result.push_back(
                {locate(host, name, location),
                    symbol,
                    Luau::toString(type),
                    documentation(source, *checked, position),
                    {}}
            );
        }

        return result;
    }

    rust::Vec<Completion>
    completion(Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position) {
        rust::Vec<Completion> result;
        const auto &name = prepared.module.name;

        auto replace = replacement(prepared.text, position);
        const auto scope = Luau::findScopeAtPosition(prepared.module, position);

        const auto native = Luau::autocomplete(
            frontend,
            name,
            position,
            [&](std::string tag, std::optional<const Luau::ExternType *>, std::optional<std::string>) {
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

        for (const auto &[label, entry] : native.entryMap) {
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

            bool snippet = false;

            if (!entry.insertText && entry.parens != Luau::ParenthesesRecommendation::None) {
                insertion += entry.parens == Luau::ParenthesesRecommendation::CursorAfter ? "()$0" : "($0)";
                snippet = true;
            }

            const auto type = entry.type ? Luau::follow(*entry.type) : nullptr;
            const bool is_function = type && (Luau::get<Luau::FunctionType>(type) || Luau::isOverloadedFunction(type));

            result.push_back(
                {label,
                    completion_kind(entry.kind),
                    is_function,
                    entry.type ? Luau::toString(*entry.type) : std::string{},
                    entry.documentationSymbol.value_or(""),
                    {},
                    insertion,
                    snippet,
                    locate(host, name, candidate_replacement),
                    entry.replaceDotWithColon,
                    range(operation),
                    entry.deprecated}
            );
        }

        return result;
    }

    rust::Vec<Signature> signatures(const Host &host, const Prepared &prepared, Luau::Position position) {
        rust::Vec<Signature> result;
        const auto *checked = &prepared.module;
        const auto &source = prepared.source;
        const auto &name = checked->name;

        const auto cursor = offset(prepared.text, position);
        const auto ancestry = Luau::findAncestryAtPositionForAutocomplete(source, position);
        Luau::AstExprCall *call = nullptr;

        for (auto *node : ancestry) {
            if (auto *candidate = node->as<Luau::AstExprCall>()) {
                if (!candidate->func->location.contains(position) && candidate->argLocation.containsClosed(position) &&
                    (position != candidate->argLocation.end || !completed(prepared, *candidate))) {
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

        for (const auto type : overloads(call_type(*checked, call))) {
            const auto *function = Luau::get<Luau::FunctionType>(type);
            Luau::ToStringOptions display;
            display.functionTypeArguments = true;
            display.hideFunctionSelfArgument = call->self;
            rust::Vec<Parameter> parameters;
            const auto [arguments, tail] = Luau::flatten(function->argTypes);
            const std::size_t skip = call->self && !arguments.empty() ? 1 : 0;

            for (std::size_t index = skip; index < arguments.size(); ++index) {
                const std::string argument_name = index < function->argNames.size() && function->argNames[index]
                                                      ? function->argNames[index]->name
                                                      : std::string{};

                parameters.push_back({argument_name, Luau::toString(arguments[index]), {}, false});
            }

            if (tail) {
                const auto *variadic = Luau::get<Luau::VariadicTypePack>(Luau::follow(*tail));

                if (!variadic || !variadic->hidden) {
                    parameters.push_back({"...", Luau::toString(*tail), {}, true});
                }
            }

            result.push_back(
                {name,
                    Luau::toString(type, display),
                    documentation(source, *checked, call->func->location.begin),
                    {},
                    std::move(parameters),
                    active,
                    resolved && Luau::follow(*resolved) == type}
            );
        }

        return result;
    }

    rust::Vec<SourceLocation> definitions(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position, bool types
    ) {
        rust::Vec<SourceLocation> result;
        const auto *checked = &prepared.module;
        const auto &source = prepared.source;
        const auto &name = checked->name;

        offset(prepared.text, position);
        const auto indexed = index(frontend, *checked, source);

        if (const auto *entry = at(indexed, position)) {
            if (!types || entry->type_namespace) {
                result.push_back(locate(host, entry->declaration.identity, entry->declaration.location));

                return result;
            }
        }

        if (!types) {
            const auto trace = frontend.requireTrace.find(name);
            bool found = false;

            if (trace != frontend.requireTrace.end()) {
                for (const auto &[node, information] : trace->second.exprs) {
                    const auto *call = node->as<Luau::AstExprCall>();
                    const auto *global = call ? call->func->as<Luau::AstExprGlobal>() : nullptr;

                    if (call && global && global->name == "require" && call->args.size && !information.name.empty() &&
                        call->args.data[0]->location.contains(position)) {
                        result.push_back(locate(host, information.name, {{0, 0}, {0, 0}}));
                        found = true;
                    }
                }
            }

            if (found) {
                return result;
            }
        }

        auto expression = Luau::findExprOrLocalAtPosition(source, position);
        Luau::TypeId type = nullptr;

        if (auto *local = expression.getLocal()) {
            type = local_type(*checked, local);

            if (types) {
                if (const auto target = annotation_declaration(frontend, *checked, local->annotation)) {
                    result.push_back(locate(host, target->identity, target->location));

                    return result;
                }
            }
        } else if (const auto *node = expression.getExpr()) {
            if (types) {
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
            throw std::invalid_argument("rename target has no semantic declaration");
        }

        if (!entry->editable) {
            throw std::invalid_argument("rename is unavailable for this source representation");
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
            throw std::invalid_argument("rename validation requires complete candidate syntax and binding information");
        }

        const auto indexed = index(frontend, module, source);
        const auto kind = targets.front().kind;
        const bool type_namespace = targets.front().type_namespace;

        const bool affected = std::any_of(targets.begin(), targets.end(), [&](const Entry &target) {
            return target.declaration.identity == module.name ||
                   std::any_of(indexed.entries.begin(), indexed.entries.end(), [&](const Entry &entry) {
                       return same(target.declaration, entry.declaration) &&
                              target.type_namespace == entry.type_namespace;
                   });
        });

        if ((affected || kind == BindingKind::Property) && name != targets.front().name) {
            if (kind == BindingKind::Property) {
                if (indexed.properties.count(name)) {
                    throw std::invalid_argument("rename would conflict with an existing property");
                }
            } else {
                if (indexed.names.count(name) && (!type_namespace || type_collision(source, indexed, name))) {
                    throw std::invalid_argument("rename would conflict with an existing value or type binding");
                }

                for (const auto &[location, scope] : module.scopes) {
                    if ((!type_namespace && scope->linearSearchForBinding(name)) || scope->lookupType(name) ||
                        scope->lookupPack(name)) {
                        throw std::invalid_argument("rename would capture an existing value or type binding");
                    }
                }
            }
        }

        rust::Vec<Occurrence> result;
        std::vector<std::pair<std::size_t, std::size_t>> edits;

        for (const auto &entry : indexed.entries) {
            for (std::size_t target = 0; target < targets.size(); ++target) {
                if (!same(targets[target].declaration, entry.declaration) ||
                    targets[target].type_namespace != entry.type_namespace) {
                    continue;
                }

                if (!entry.editable) {
                    throw std::invalid_argument("rename is unavailable for an occurrence source representation");
                }

                if (entry.type_namespace && name != targets.front().name) {
                    for (const auto *node : Luau::findAstAncestryOfPosition(source, entry.location.begin, true)) {
                        if (const auto *global = node->as<Luau::AstExprGlobal>();
                            global && global->location == entry.location) {
                            const auto scope = Luau::findScopeAtPosition(module, entry.location.begin);

                            if (scope && scope->linearSearchForBinding(name)) {
                                throw std::invalid_argument(
                                    "rename would capture an existing type-function value binding"
                                );
                            }
                        }
                    }
                }

                if (name == "typeof" && entry.role == OccurrenceRole::Declaration && entry.type_namespace) {
                    for (const auto *node : Luau::findAstAncestryOfPosition(source, entry.location.begin, true)) {
                        const auto *alias = node->as<Luau::AstStatTypeAlias>();
                        const auto *function = node->as<Luau::AstStatTypeFunction>();

                        if ((alias && alias->nameLocation == entry.location) ||
                            (function && function->nameLocation == entry.location)) {
                            throw std::invalid_argument("type declarations cannot be named typeof");
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
                throw std::invalid_argument("rename is not valid in every source context");
            }
        }

        return result;
    }

    rust::Vec<Hint> hints(const Host &host, const Luau::Module &module, const Luau::SourceModule &source) {
        Hints visitor(host, module);
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
}
