#include "editor.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/Module.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"

#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace instar {
    namespace {
        std::optional<std::string_view> view(Text value) {
            if (!value.data && value.length != 0) {
                return std::nullopt;
            }

            return std::string_view(value.data ? reinterpret_cast<const char *>(value.data) : "", value.length);
        }

        Text text(std::string_view value) {
            return {reinterpret_cast<const uint8_t *>(value.data()), value.size()};
        }

        Location location(const Luau::Location &value) {
            return {value.begin.line, value.begin.column, value.end.line, value.end.column};
        }

        struct ModuleContext {
            const Luau::SourceModule *source;
            Luau::ModulePtr module;
        };

        std::optional<ModuleContext> module_context(Luau::Frontend &frontend, Text name) {
            const std::optional<std::string_view> value = view(name);

            if (!value) {
                return std::nullopt;
            }

            const std::string module_name(*value);
            const Luau::SourceModule *source = frontend.getSourceModule(module_name);
            const Luau::ModulePtr module = frontend.moduleResolver.getModule(module_name);

            if (!source || !module || !source->root) {
                return std::nullopt;
            }

            return ModuleContext{source, module};
        }

        std::optional<Luau::AstType *> type_node_at(const Luau::SourceModule &source, Luau::Position position) {
            struct Finder final : Luau::AstVisitor {
                explicit Finder(Luau::Position position) : position(position) {}

                bool visit(Luau::AstNode *node) override { return node->location.contains(position); }

                bool visit(Luau::AstType *type) override {
                    if (type->location.contains(position) && (!result || result->location.encloses(type->location))) {
                        result = type;
                    }

                    return true;
                }

                bool visit(Luau::AstTypePack *) override { return true; }

                Luau::Position position;
                Luau::AstType *result = nullptr;
            } finder(position);

            source.root->visit(&finder);

            return finder.result ? std::optional<Luau::AstType *>(finder.result) : std::nullopt;
        }

        std::optional<Luau::TypeId> type_at(const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position) {
            if (const std::optional<Luau::AstType *> type = type_node_at(source, position)) {
                if (const Luau::TypeId *resolved = module.astResolvedTypes.find(*type)) {
                    return *resolved;
                }
            }

            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);

            if (Luau::AstExpr *expression = target.getExpr()) {
                if (const Luau::TypeId *type = module.astTypes.find(expression)) {
                    return *type;
                }

                if (const Luau::TypePackId *pack = module.astTypePacks.find(expression)) {
                    return Luau::first(*pack);
                }
            }

            if (Luau::AstLocal *local = target.getLocal()) {
                if (const std::optional<Luau::Binding> binding = Luau::findBindingAtPosition(module, source, position)) {
                    return binding->typeId;
                }

                Luau::ScopePtr scope = Luau::findScopeAtPosition(module, position);

                while (scope) {
                    const auto found = scope->bindings.find(Luau::Symbol(local));

                    if (found != scope->bindings.end()) {
                        return found->second.typeId;
                    }

                    scope = scope->parent;
                }
            }

            return std::nullopt;
        }

        std::optional<std::string> expression_name(const Luau::AstExpr *expression) {
            if (const auto *local = expression->as<Luau::AstExprLocal>()) {
                return std::string(local->local->name.value);
            }

            if (const auto *global = expression->as<Luau::AstExprGlobal>()) {
                return std::string(global->name.value);
            }

            if (const auto *index = expression->as<Luau::AstExprIndexName>()) {
                return std::string(index->index.value);
            }

            return std::nullopt;
        }

        std::optional<std::string> target_name(Luau::ExprOrLocal &target) {
            if (const Luau::AstLocal *local = target.getLocal()) {
                return std::string(local->name.value);
            }

            if (const Luau::AstExpr *expression = target.getExpr()) {
                if (const auto *index = expression->as<Luau::AstExprIndexName>()) {
                    if (const std::optional<std::string> base = expression_name(index->expr)) {
                        return *base + (index->op == ':' ? ":" : ".") + std::string(index->index.value);
                    }
                }

                return expression_name(expression);
            }

            return std::nullopt;
        }

        std::optional<Luau::Location> target_location(Luau::ExprOrLocal &target) {
            if (const Luau::AstLocal *local = target.getLocal()) {
                return local->location;
            }

            if (const Luau::AstExpr *expression = target.getExpr()) {
                if (const auto *index = expression->as<Luau::AstExprIndexName>()) {
                    return index->indexLocation;
                }

                return expression->location;
            }

            return std::nullopt;
        }

        std::string type_text(Luau::TypeId type) {
            return Luau::toString(type, Luau::ToStringOptions{});
        }

        std::optional<Luau::ModuleName> type_definition_module(const Luau::Module &module, Luau::TypeId type, std::optional<Luau::Location> &definition) {
            type = Luau::follow(type);

            if (const auto *function = Luau::get<Luau::FunctionType>(type); function && function->definition) {
                const Luau::FunctionDefinition &value = *function->definition;
                definition = value.definitionLocation;

                return value.definitionModuleName.value_or(module.name);
            }

            if (const auto *external = Luau::get<Luau::ExternType>(type); external && external->definitionLocation) {
                definition = *external->definitionLocation;

                return external->definitionModuleName;
            }

            if (const auto *table = Luau::get<Luau::TableType>(type); table && table->definitionLocation.begin.hasValue()) {
                definition = table->definitionLocation;

                return table->definitionModuleName.empty() ? module.name : table->definitionModuleName;
            }

            return std::nullopt;
        }

        std::optional<std::pair<Luau::ModuleName, Luau::Location>>
        type_alias_definition(Luau::Frontend &frontend, const ModuleContext &context, const Luau::AstTypeReference *reference) {
            Luau::ScopePtr scope = Luau::findScopeAtPosition(*context.module, reference->location.begin);

            if (!scope) {
                return std::nullopt;
            }

            std::optional<Luau::TypeFun> alias;

            if (reference->prefix) {
                alias = scope->lookupImportedType(reference->prefix->value, reference->name.value);
            } else {
                alias = scope->lookupType(reference->name.value);
            }

            if (!alias || !alias->definitionLocation) {
                return std::nullopt;
            }

            Luau::ModuleName module_name = context.source->name;

            if (reference->prefix) {
                for (Luau::ScopePtr current = scope; current; current = current->parent) {
                    const auto found = current->importedModules.find(reference->prefix->value);

                    if (found != current->importedModules.end()) {
                        module_name = found->second;
                        break;
                    }
                }
            }

            const Luau::SourceModule *source = frontend.getSourceModule(module_name);

            if (!source || !source->root) {
                return std::nullopt;
            }

            Luau::AstNode *node = Luau::findNodeAtPosition(*source, alias->definitionLocation->begin);
            const auto *declaration = node ? node->as<Luau::AstStatTypeAlias>() : nullptr;

            if (!declaration || declaration->location != *alias->definitionLocation) {
                return std::nullopt;
            }

            return std::pair<Luau::ModuleName, Luau::Location>(module_name, declaration->nameLocation);
        }

        struct BindingIdentity {
            const Luau::Binding *binding;
        };

        std::optional<BindingIdentity> binding_identity(const ModuleContext &context, const Luau::AstExprGlobal *expression) {
            const Luau::ScopePtr scope = Luau::findScopeAtPosition(*context.module, expression->location.begin);

            if (!scope) {
                return std::nullopt;
            }

            const std::optional<std::pair<Luau::Binding *, Luau::Scope *>> binding = scope->lookupEx(Luau::Symbol(expression->name));

            return binding ? std::optional<BindingIdentity>{{binding->first}} : std::nullopt;
        }

        struct PropertyIdentity {
            Luau::ModuleName module;
            Luau::Location location;
            const Luau::Property *property;
        };

        bool same_identity(const BindingIdentity &left, const BindingIdentity &right);
        bool same_identity(const PropertyIdentity &left, const PropertyIdentity &right);

        void collect_property_identities(
            const Luau::Module &module, Luau::TypeId type, std::string_view name, std::vector<PropertyIdentity> &identities, std::vector<Luau::TypeId> &seen,
            bool &unknown
        ) {
            type = Luau::follow(type);

            for (Luau::TypeId candidate : seen) {
                if (candidate == type) {
                    return;
                }
            }

            seen.push_back(type);

            if (const auto *table = Luau::get<Luau::TableType>(type)) {
                if (table->boundTo) {
                    collect_property_identities(module, *table->boundTo, name, identities, seen, unknown);

                    return;
                }

                const auto found = table->props.find(std::string(name));

                if (found != table->props.end()) {
                    const std::optional<Luau::Location> property_location = found->second.location ? found->second.location : found->second.typeLocation;

                    if (!property_location) {
                        unknown = true;
                    } else {
                        identities.push_back(
                            {
                                table->definitionModuleName.empty() ? module.name : table->definitionModuleName,
                                *property_location,
                                &found->second,
                            }
                        );
                    }
                }
            }

            if (const auto *external = Luau::get<Luau::ExternType>(type)) {
                const auto found = external->props.find(std::string(name));

                if (found != external->props.end()) {
                    const std::optional<Luau::Location> property_location = found->second.location ? found->second.location : found->second.typeLocation;

                    if (!property_location) {
                        unknown = true;
                    } else {
                        identities.push_back({external->definitionModuleName.empty() ? module.name : external->definitionModuleName, *property_location, &found->second});
                    }
                } else if (external->parent) {
                    collect_property_identities(module, *external->parent, name, identities, seen, unknown);
                }
            }

            if (const auto *metatable = Luau::get<Luau::MetatableType>(type)) {
                collect_property_identities(module, metatable->table, name, identities, seen, unknown);

                if (const auto *meta = Luau::get<Luau::TableType>(Luau::follow(metatable->metatable))) {
                    const auto index = meta->props.find("__index");

                    if (index != meta->props.end() && index->second.readTy) {
                        collect_property_identities(module, *index->second.readTy, name, identities, seen, unknown);
                    }
                }
            }

            if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                for (Luau::TypeId option : union_type->options) {
                    collect_property_identities(module, option, name, identities, seen, unknown);
                }
            }

            if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (Luau::TypeId part : intersection->parts) {
                    collect_property_identities(module, part, name, identities, seen, unknown);
                }
            }
        }

        std::optional<PropertyIdentity> property_identity(const Luau::Module &module, Luau::TypeId base, std::string_view name) {
            std::vector<PropertyIdentity> identities;
            std::vector<Luau::TypeId> seen;
            bool unknown = false;
            collect_property_identities(module, base, name, identities, seen, unknown);

            if (unknown || identities.empty()) {
                return std::nullopt;
            }

            const PropertyIdentity &first = identities.front();

            for (size_t index = 1; index < identities.size(); ++index) {
                if (!same_identity(first, identities[index])) {
                    return std::nullopt;
                }
            }

            return first;
        }

        bool same_identity(const BindingIdentity &left, const BindingIdentity &right) {
            return left.binding == right.binding;
        }

        bool same_identity(const PropertyIdentity &left, const PropertyIdentity &right) {
            return left.module == right.module && left.location == right.location;
        }

        struct LocalOccurrence {
            Luau::Location range;
            bool declaration;
        };

        struct IdentityReferenceVisitor final : Luau::AstVisitor {
            IdentityReferenceVisitor(const ModuleContext &context, const std::optional<BindingIdentity> &binding, const std::optional<PropertyIdentity> &property)
                : context(context), binding(binding), property(property) {}

            bool visit(Luau::AstExprGlobal *expression) override {
                if (binding) {
                    const std::optional<BindingIdentity> occurrence = binding_identity(context, expression);

                    if (occurrence && same_identity(*binding, *occurrence)) {
                        references.push_back({expression->location, false});
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprIndexName *expression) override {
                if (property) {
                    const Luau::TypeId *base = context.module->astTypes.find(expression->expr);
                    const std::optional<PropertyIdentity> occurrence = base ? property_identity(*context.module, *base, expression->index.value) : std::nullopt;

                    bool declaration = false;

                    for (const Luau::AstExprIndexName *candidate : declaration_properties) {
                        declaration = candidate == expression;

                        if (declaration) {
                            break;
                        }
                    }

                    if (occurrence && same_identity(*property, *occurrence)) {
                        references.push_back({expression->indexLocation, declaration});
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatFunction *statement) override {
                if (const auto *index = statement->name->as<Luau::AstExprIndexName>()) {
                    declaration_properties.push_back(index);
                }

                return true;
            }

            const ModuleContext &context;
            const std::optional<BindingIdentity> &binding;
            const std::optional<PropertyIdentity> &property;
            std::vector<const Luau::AstExprIndexName *> declaration_properties;
            std::vector<LocalOccurrence> references;
        };

        std::optional<Luau::AstLocal *> target_local(const Luau::SourceModule &source, Luau::Position position) {
            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);

            if (Luau::AstLocal *local = target.getLocal()) {
                return local;
            }

            if (Luau::AstExpr *expression = target.getExpr()) {
                if (const auto *local = expression->as<Luau::AstExprLocal>()) {
                    return local->local;
                }
            }

            return std::nullopt;
        }

        struct LocalVisitor final : Luau::AstVisitor {
            explicit LocalVisitor(Luau::AstLocal *target) : target(target) {}

            bool visit(Luau::AstExprLocal *expression) override {
                if (expression->local == target) {
                    occurrences.push_back({expression->location, false});
                }

                return true;
            }

            bool visit(Luau::AstStatLocal *statement) override {
                for (Luau::AstLocal *local : statement->vars) {
                    if (local == target) {
                        occurrences.push_back({local->location, true});
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *statement) override {
                if (statement->name == target) {
                    occurrences.push_back({target->location, true});
                }

                return true;
            }

            bool visit(Luau::AstStatFor *statement) override {
                if (statement->var == target) {
                    occurrences.push_back({target->location, true});
                }

                return true;
            }

            bool visit(Luau::AstStatForIn *statement) override {
                for (Luau::AstLocal *local : statement->vars) {
                    if (local == target) {
                        occurrences.push_back({target->location, true});
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *function) override {
                if (function->self == target) {
                    occurrences.push_back({target->location, true});
                }

                for (Luau::AstLocal *local : function->args) {
                    if (local == target) {
                        occurrences.push_back({target->location, true});
                    }
                }

                return true;
            }

            Luau::AstLocal *target;
            std::vector<LocalOccurrence> occurrences;
        };

        struct TokenVisitor final : Luau::AstVisitor {
            bool visit(Luau::AstExprGlobal *expression) override {
                tokens.push_back({location(expression->location), SemanticTokenNamespace, 0, false});

                return true;
            }

            bool visit(Luau::AstExprLocal *expression) override {
                tokens.push_back({location(expression->location), SemanticTokenVariable, 0, false});

                return true;
            }

            bool visit(Luau::AstStatLocal *statement) override {
                const uint32_t modifiers = statement->isConst ? 1 : 0;

                for (Luau::AstLocal *local : statement->vars) {
                    tokens.push_back({location(local->location), SemanticTokenVariable, modifiers, true});
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *statement) override {
                tokens.push_back({location(statement->name->location), SemanticTokenFunction, 0, true});

                return true;
            }

            bool visit(Luau::AstExprIndexName *expression) override {
                tokens.push_back({location(expression->indexLocation), SemanticTokenProperty, 0, false});

                return true;
            }

            bool visit(Luau::AstExprCall *call) override {
                if (call->self) {
                    if (const auto *index = call->func->as<Luau::AstExprIndexName>()) {
                        methods.push_back(index->indexLocation);
                    }
                }

                return true;
            }

            bool visit(Luau::AstTypeReference *reference) override {
                tokens.push_back({location(reference->location), SemanticTokenType, 0, false});

                return true;
            }

            bool visit(Luau::AstExprFunction *function) override {
                for (Luau::AstLocal *local : function->args) {
                    tokens.push_back({location(local->location), SemanticTokenParameter, 0, true});
                }

                return true;
            }

            bool visit(Luau::AstGenericType *generic) override {
                tokens.push_back({location(generic->location), SemanticTokenTypeParameter, 0, true});

                return true;
            }

            bool visit(Luau::AstGenericTypePack *generic) override {
                tokens.push_back({location(generic->location), SemanticTokenTypeParameter, 0, true});

                return true;
            }

            bool visit(Luau::AstStatTypeAlias *statement) override {
                tokens.push_back({location(statement->nameLocation), SemanticTokenClass, 0, true});

                return true;
            }

            std::vector<EditorSemanticToken> tokens;
            std::vector<Luau::Location> methods;
        };

        struct CallFinder final : Luau::AstVisitor {
            explicit CallFinder(Luau::Position position) : position(position) {}

            bool visit(Luau::AstExprCall *call) override {
                if (call->location.contains(position) && (!result || result->location.encloses(call->location))) {
                    result = call;
                }

                return true;
            }

            Luau::Position position;
            Luau::AstExprCall *result = nullptr;
        };

        std::optional<Luau::TypeId> call_type(const Luau::Module &module, Luau::AstExprCall *call) {
            if (const Luau::TypeId *type = module.astOverloadResolvedTypes.find(call)) {
                return *type;
            }

            if (const Luau::TypeId *type = module.astOriginalCallTypes.find(call)) {
                return *type;
            }

            return std::nullopt;
        }

        EditorCompletionKind completion_kind(Luau::AutocompleteEntryKind kind) {
            switch (kind) {
            case Luau::AutocompleteEntryKind::Property:
                return CompletionProperty;

            case Luau::AutocompleteEntryKind::Binding:
                return CompletionVariable;

            case Luau::AutocompleteEntryKind::Keyword:
                return CompletionKeyword;

            case Luau::AutocompleteEntryKind::String:
                return CompletionValue;

            case Luau::AutocompleteEntryKind::Type:
                return CompletionTypeParameter;

            case Luau::AutocompleteEntryKind::Module:
                return CompletionModule;

            case Luau::AutocompleteEntryKind::GeneratedFunction:
                return CompletionFunction;

            case Luau::AutocompleteEntryKind::RequirePath:
                return CompletionFile;

            case Luau::AutocompleteEntryKind::HotComment:
                return CompletionText;
            }

            return CompletionText;
        }

        std::string type_pack_text(Luau::TypePackId pack) {
            Luau::ToStringOptions options;
            options.functionTypeArguments = true;
            options.hideFunctionSelfArgument = true;

            return Luau::toString(pack, options);
        }

        std::vector<std::string> function_parameters(const Luau::FunctionType &function) {
            const auto [parameters, tail] = Luau::flatten(function.argTypes);
            const size_t offset = function.hasSelf && !parameters.empty() ? 1 : 0;
            std::vector<std::string> result;

            for (size_t index = offset; index < parameters.size(); ++index) {
                std::string value;

                if (index < function.argNames.size() && function.argNames[index]) {
                    value += function.argNames[index]->name;
                    value += ": ";
                }

                value += type_text(parameters[index]);
                result.push_back(std::move(value));
            }

            if (tail) {
                result.push_back("...");
            }

            return result;
        }

        int32_t emit_local_symbols(const Luau::SourceModule &source, Luau::AstLocal *target, SymbolCallback callback, void *context) {
            LocalVisitor visitor(target);
            source.root->visit(&visitor);
            const Text symbol_name = text(target->name.value);
            const Text path = text(source.name);

            for (const LocalOccurrence &occurrence : visitor.occurrences) {
                const Location range = location(occurrence.range);
                const EditorSymbol result{symbol_name, path, range, range, SymbolVariable, 0, uint8_t(occurrence.declaration)};

                if (!callback(context, &result)) {
                    return StatusCallbackFailure;
                }
            }

            return StatusSuccess;
        }

        int32_t no_result() {
            return StatusSuccess;
        }
    } // namespace

    int32_t editor_hover(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, HoverCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const Luau::Position position(line, column);
        const std::optional<Luau::TypeId> type = type_at(*context_value->module, *context_value->source, position);

        if (!type) {
            return StatusSuccess;
        }

        Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(*context_value->source, position);
        const std::optional<std::string> name_value = target_name(target);
        const std::optional<Luau::Location> range = target_location(target);
        const std::string type_value = type_text(*type);
        const std::optional<std::string> documentation = Luau::getDocumentationSymbolAtPosition(*context_value->source, *context_value->module, position);
        const Text name_text = name_value ? text(*name_value) : Text{};
        const Text type_output = text(type_value);
        const Text documentation_text = documentation ? text(*documentation) : Text{};
        const EditorHover result{name_text, type_output, documentation_text, uint8_t(range.has_value()), range ? location(*range) : Location{}};

        return callback(context, &result) ? StatusSuccess : StatusCallbackFailure;
    }

    int32_t emit_completion_items(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, CompletionCallback callback, void *context) {

        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const std::optional<std::string_view> module_name = view(name);

        const Luau::AutocompleteResult result = Luau::autocomplete(
            frontend,
            std::string(*module_name),
            Luau::Position(line, column),
            [](std::string, std::optional<const Luau::ExternType *>, std::optional<std::string>) { return std::optional<Luau::AutocompleteEntryMap>(); }
        );

        for (const auto &[candidate_name, candidate] : result.entryMap) {

            const std::string detail = candidate.type ? type_text(*candidate.type) : std::string();
            const Text candidate_name_text = text(candidate_name);
            const Text detail_text = text(detail);
            const Text documentation = candidate.documentationSymbol ? text(*candidate.documentationSymbol) : Text{};
            const Text insert = candidate.insertText ? text(*candidate.insertText) : Text{};

            const EditorCompletionItem item{
                candidate_name_text,
                detail_text,
                documentation,
                insert,
                0,
                {},
                candidate.kind == Luau::AutocompleteEntryKind::RequirePath ? CompletionRequirePath : CompletionNormal,
                completion_kind(candidate.kind),
                uint8_t(candidate.deprecated),
            };

            if (!callback(context, &item)) {
                return StatusCallbackFailure;
            }
        }

        return StatusSuccess;
    }

    int32_t editor_completion(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, CompletionCallback callback, void *context) {
        return emit_completion_items(frontend, name, line, column, callback, context);
    }

    int32_t editor_signature_help(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, SignatureCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        CallFinder finder(Luau::Position(line, column));
        context_value->source->root->visit(&finder);

        if (!finder.result) {
            return StatusSuccess;
        }

        const std::optional<Luau::TypeId> call = call_type(*context_value->module, finder.result);

        if (!call) {
            return StatusSuccess;
        }

        const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*call));

        if (!function) {
            return StatusSuccess;
        }

        const std::vector<std::string> parameters = function_parameters(*function);
        const std::string label = type_text(*call);
        std::vector<Text> parameter_text;
        parameter_text.reserve(parameters.size());

        for (const std::string &parameter : parameters) {
            parameter_text.push_back(text(parameter));
        }

        uint32_t active = 0;

        for (Luau::AstExpr *argument : finder.result->args) {
            if (argument->location.begin >= Luau::Position(line, column)) {
                break;
            }

            ++active;
        }

        const EditorSignatureHelp result{text(label), parameter_text.data(), parameter_text.size(), uint8_t(true), active};

        return callback(context, &result) ? StatusSuccess : StatusCallbackFailure;
    }

    int32_t editor_type_hints(Luau::Frontend &frontend, Text name, HintCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        struct HintVisitor final : Luau::AstVisitor {
            explicit HintVisitor(const Luau::Module &module) : module(module) {}

            bool visit(Luau::AstStatLocal *statement) override {
                for (size_t index = 0; index < statement->vars.size && index < statement->values.size; ++index) {
                    Luau::AstLocal *local = statement->vars.data[index];

                    if (local->annotation && local->annotation->location != Luau::Location()) {
                        continue;
                    }

                    if (const Luau::TypeId *type = module.astTypes.find(statement->values.data[index])) {
                        hints.push_back({local->location, type_text(*type)});
                    } else if (const Luau::TypePackId *pack = module.astTypePacks.find(statement->values.data[index])) {
                        if (const std::optional<Luau::TypeId> type = Luau::first(*pack)) {
                            hints.push_back({local->location, type_text(*type)});
                        }
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprCall *call) override {
                const Luau::TypeId *type = module.astOverloadResolvedTypes.find(call);

                if (!type) {
                    type = module.astOriginalCallTypes.find(call);
                }

                if (!type) {
                    return true;
                }

                const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*type));

                if (!function) {
                    return true;
                }

                const auto [parameters, tail] = Luau::flatten(function->argTypes);
                const size_t offset = function->hasSelf && !parameters.empty() ? 1 : 0;

                for (size_t index = 0; index < call->args.size; ++index) {
                    const size_t parameter = index + offset;

                    if (parameter >= function->argNames.size() || !function->argNames[parameter]) {
                        continue;
                    }

                    hints.push_back(
                        {
                            Luau::Location(call->args.data[index]->location.begin, call->args.data[index]->location.begin),
                            type_text(parameters[parameter]),
                        }
                    );
                }

                return true;
            }

            const Luau::Module &module;
            std::vector<std::pair<Luau::Location, std::string>> hints;
        } visitor(*context_value->module);

        context_value->source->root->visit(&visitor);

        for (const auto &[range, type] : visitor.hints) {
            const EditorTypeHint result{location(range), text(type)};

            if (!callback(context, &result)) {
                return StatusCallbackFailure;
            }
        }

        return StatusSuccess;
    }

    int32_t editor_semantic_tokens(Luau::Frontend &frontend, Text name, TokenCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        TokenVisitor visitor;
        context_value->source->root->visit(&visitor);

        for (const EditorSemanticToken &token : visitor.tokens) {
            bool method = false;

            if (token.kind == SemanticTokenProperty) {
                for (const Luau::Location &candidate : visitor.methods) {
                    const Location range = location(candidate);

                    if (range.begin_line == token.range.begin_line && range.begin_column == token.range.begin_column && range.end_line == token.range.end_line &&
                        range.end_column == token.range.end_column) {
                        method = true;
                        break;
                    }
                }
            }

            if (method) {
                EditorSemanticToken value = token;
                value.kind = SemanticTokenMethod;

                if (!callback(context, &value)) {
                    return StatusCallbackFailure;
                }
            } else if (!callback(context, &token)) {
                return StatusCallbackFailure;
            }
        }

        return StatusSuccess;
    }

    int32_t emit_type_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const Luau::Position position(line, column);
        const std::optional<Luau::AstLocal *> local = target_local(*context_value->source, position);

        if (local) {
            const std::optional<Luau::TypeId> type = type_at(*context_value->module, *context_value->source, position);

            if (!type) {
                return StatusSuccess;
            }

            std::optional<Luau::Location> definition;
            const std::optional<Luau::ModuleName> module_name = type_definition_module(*context_value->module, *type, definition);

            if (!module_name || !definition) {
                return StatusSuccess;
            }

            const Location range = location(*definition);
            const EditorNavigation result{text(*module_name), range, range};

            return callback(context, &result) ? StatusSuccess : StatusCallbackFailure;
        }

        Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(*context_value->source, position);

        if (Luau::AstExpr *expression = target.getExpr()) {
            if (auto *index = expression->as<Luau::AstExprIndexName>()) {
                const Luau::TypeId *base = context_value->module->astTypes.find(index->expr);

                if (base) {
                    if (const auto origin = property_identity(*context_value->module, *base, index->index.value)) {
                        const Luau::Property *property = origin->property;
                        std::optional<Luau::Location> definition;

                        const std::optional<Luau::ModuleName> module_name =
                            property->readTy ? type_definition_module(*context_value->module, *property->readTy, definition) : std::nullopt;

                        const Luau::Location range = definition.value_or(origin->location);
                        const std::string path = module_name.value_or(origin->module);
                        const EditorNavigation result{text(path), location(range), location(range)};

                        if (!callback(context, &result)) {
                            return StatusCallbackFailure;
                        }

                        return StatusSuccess;
                    }
                }
            }
        }

        if (const std::optional<Luau::AstType *> type = type_node_at(*context_value->source, position)) {
            if (const auto *reference = (*type)->as<Luau::AstTypeReference>()) {
                if (const std::optional<std::pair<Luau::ModuleName, Luau::Location>> alias = type_alias_definition(frontend, *context_value, reference)) {
                    const Location range = location(alias->second);
                    const EditorNavigation result{text(alias->first), range, range};

                    if (!callback(context, &result)) {
                        return StatusCallbackFailure;
                    }

                    return StatusSuccess;
                }
            }
        }

        if (const std::optional<Luau::TypeId> type = type_at(*context_value->module, *context_value->source, position)) {
            std::optional<Luau::Location> definition;
            const std::optional<Luau::ModuleName> module_name = type_definition_module(*context_value->module, *type, definition);

            if (module_name && definition) {
                const Location range = location(*definition);
                const EditorNavigation result{text(*module_name), range, range};

                if (!callback(context, &result)) {
                    return StatusCallbackFailure;
                }
            }
        }

        return StatusSuccess;
    }

    int32_t emit_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const std::optional<Luau::AstLocal *> local = target_local(*context_value->source, Luau::Position(line, column));

        if (local) {
            LocalVisitor visitor(*local);
            context_value->source->root->visit(&visitor);

            for (const LocalOccurrence &occurrence : visitor.occurrences) {
                if (!occurrence.declaration) {
                    continue;
                }

                const Location range = location(occurrence.range);
                const EditorNavigation result{text(context_value->source->name), range, range};

                if (!callback(context, &result)) {
                    return StatusCallbackFailure;
                }
            }

            return StatusSuccess;
        }

        return emit_type_definition(frontend, name, line, column, callback, context);
    }

    int32_t editor_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context) {
        return emit_definition(frontend, name, line, column, callback, context);
    }

    int32_t editor_declaration(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const Luau::Position position(line, column);
        const std::optional<Luau::AstLocal *> local = target_local(*context_value->source, position);

        if (local) {
            return emit_definition(frontend, name, line, column, callback, context);
        }

        Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(*context_value->source, position);

        if (Luau::AstExpr *expression = target.getExpr()) {
            if (auto *index = expression->as<Luau::AstExprIndexName>()) {
                const Luau::TypeId *base = context_value->module->astTypes.find(index->expr);

                if (base) {
                    if (const auto origin = property_identity(*context_value->module, *base, index->index.value)) {
                        const Location result_range = location(origin->location);
                        const EditorNavigation result{text(origin->module), result_range, result_range};

                        return callback(context, &result) ? StatusSuccess : StatusCallbackFailure;
                    }
                }
            }
        }

        if (const std::optional<Luau::AstType *> type = type_node_at(*context_value->source, position)) {
            if (const auto *reference = (*type)->as<Luau::AstTypeReference>()) {
                if (const std::optional<std::pair<Luau::ModuleName, Luau::Location>> alias = type_alias_definition(frontend, *context_value, reference)) {
                    const Location result_range = location(alias->second);
                    const EditorNavigation result{text(alias->first), result_range, result_range};

                    return callback(context, &result) ? StatusSuccess : StatusCallbackFailure;
                }
            }
        }

        return StatusSuccess;
    }

    int32_t editor_type_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context) {
        return emit_type_definition(frontend, name, line, column, callback, context);
    }

    int32_t editor_references(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, ReferenceCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const std::optional<Luau::AstLocal *> target = target_local(*context_value->source, Luau::Position(line, column));

        if (target) {
            LocalVisitor visitor(*target);
            context_value->source->root->visit(&visitor);

            for (const LocalOccurrence &occurrence : visitor.occurrences) {
                const EditorReference result{text(context_value->source->name), location(occurrence.range), uint8_t(occurrence.declaration)};

                if (!callback(context, &result)) {
                    return StatusCallbackFailure;
                }
            }

            return StatusSuccess;
        }

        Luau::ExprOrLocal expression = Luau::findExprOrLocalAtPosition(*context_value->source, Luau::Position(line, column));
        std::optional<BindingIdentity> binding;
        std::optional<PropertyIdentity> property;

        if (Luau::AstExpr *target_expression = expression.getExpr()) {
            if (const auto *global = target_expression->as<Luau::AstExprGlobal>()) {
                binding = binding_identity(*context_value, global);
            } else if (const auto *index = target_expression->as<Luau::AstExprIndexName>(); index && index->indexLocation.contains(Luau::Position(line, column))) {
                if (const Luau::TypeId *base = context_value->module->astTypes.find(index->expr)) {
                    property = property_identity(*context_value->module, *base, index->index.value);
                }
            }
        }

        if (!binding && !property) {
            return StatusSuccess;
        }

        for (const auto &[module_name, source] : frontend.sourceModules) {
            if (!source || !source->root) {
                continue;
            }

            const Luau::ModulePtr module = frontend.moduleResolver.getModule(module_name);

            if (!module) {
                continue;
            }

            const ModuleContext occurrence_context{source.get(), module};
            IdentityReferenceVisitor visitor(occurrence_context, binding, property);
            source->root->visit(&visitor);

            for (const LocalOccurrence &occurrence : visitor.references) {
                const EditorReference result{text(module_name), location(occurrence.range), uint8_t(occurrence.declaration)};

                if (!callback(context, &result)) {
                    return StatusCallbackFailure;
                }
            }
        }

        return StatusSuccess;
    }

    int32_t editor_prepare(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const Luau::Position position(line, column);
        Luau::ExprOrLocal selected = Luau::findExprOrLocalAtPosition(*context_value->source, position);
        const std::optional<Luau::AstLocal *> target = target_local(*context_value->source, position);
        const std::optional<Luau::Location> range = target_location(selected);

        if (!target || !range) {
            return StatusSuccess;
        }

        const EditorSymbol result{
            text((*target)->name.value),
            text(context_value->source->name),
            location(*range),
            location(*range),
            SymbolVariable,
            0,
            uint8_t(selected.getLocal() != nullptr),
        };

        return callback(context, &result) ? StatusSuccess : StatusCallbackFailure;
    }

    int32_t editor_local_references(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context) {
        if (!callback) {
            return StatusFailure;
        }

        const std::optional<ModuleContext> context_value = module_context(frontend, name);

        if (!context_value) {
            return StatusSuccess;
        }

        const std::optional<Luau::AstLocal *> target = target_local(*context_value->source, Luau::Position(line, column));

        if (!target) {
            return StatusSuccess;
        }

        return emit_local_symbols(*context_value->source, *target, callback, context);
    }

} // namespace instar
