#include "editor.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/FileResolver.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"
#include "Luau/TypePack.h"

#include <algorithm>
#include <array>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace instar {
    namespace {

        struct EditorEntry {
            std::optional<std::string> name;
            std::optional<std::string> description;
            std::optional<std::string> documentation;
            std::optional<std::string> documentation_text;
            std::optional<std::string> path;
            std::optional<Luau::Location> range;
            std::optional<Luau::Location> selection;
            std::optional<std::array<float, 4>> color;
            std::optional<bool> imports;
            std::optional<bool> require;
            std::optional<std::array<uint32_t, 4>> caller;
            std::optional<std::array<uint32_t, 4>> container;
            std::optional<uint32_t> modifiers;
            std::optional<uint32_t> kind;
            std::optional<bool> declaration;
            std::optional<std::string> insert;
            std::optional<bool> deprecated;
            std::optional<std::string> label;
            std::optional<std::vector<std::string>> parameters;
            std::optional<uint32_t> active;
        };

        void append_json_string(std::string &output, std::string_view value) {
            output.push_back('"');

            for (const unsigned char character : value) {
                switch (character) {
                case '"':
                    output += "\\\"";
                    break;

                case '\\':
                    output += "\\\\";
                    break;

                case '\b':
                    output += "\\b";
                    break;

                case '\f':
                    output += "\\f";
                    break;

                case '\n':
                    output += "\\n";
                    break;

                case '\r':
                    output += "\\r";
                    break;

                case '\t':
                    output += "\\t";
                    break;

                default:
                    if (character < 0x20) {
                        constexpr char digits[] = "0123456789abcdef";
                        output += "\\u00";
                        output.push_back(digits[character >> 4]);
                        output.push_back(digits[character & 0xf]);
                    } else {
                        output.push_back(static_cast<char>(character));
                    }

                    break;
                }
            }

            output.push_back('"');
        }

        void append_location(std::string &output, const Luau::Location &location) {
            output += '[';
            output += std::to_string(location.begin.line);
            output += ',';
            output += std::to_string(location.begin.column);
            output += ',';
            output += std::to_string(location.end.line);
            output += ',';
            output += std::to_string(location.end.column);
            output += ']';
        }

        void append_location(std::string &output, const std::array<uint32_t, 4> &location) {
            output += '[';
            output += std::to_string(location[0]);
            output += ',';
            output += std::to_string(location[1]);
            output += ',';
            output += std::to_string(location[2]);
            output += ',';
            output += std::to_string(location[3]);
            output += ']';
        }

        class JsonWriter final {
          public:
            JsonWriter() : output("[") {}

            void add(const EditorEntry &entry) {
                if (!first) {
                    output.push_back(',');
                }

                first = false;
                first_field = true;
                output.push_back('{');

                field("name", entry.name);
                field("type", entry.description);
                field("documentation", entry.documentation);
                field("documentation_text", entry.documentation_text);
                field("path", entry.path);
                field("range", entry.range);
                field("selection", entry.selection);
                field("color", entry.color);
                field("imports", entry.imports);
                field("require", entry.require);
                field("caller", entry.caller);
                field("container", entry.container);
                field("modifiers", entry.modifiers);
                field("kind", entry.kind);
                field("declaration", entry.declaration);
                field("insert", entry.insert);
                field("deprecated", entry.deprecated);
                field("label", entry.label);
                field("parameters", entry.parameters);
                field("active", entry.active);

                output.push_back('}');
            }

            std::string finish() {
                output.push_back(']');

                return std::move(output);
            }

          private:
            void key(std::string_view name) {
                if (!first_field) {
                    output.push_back(',');
                }

                first_field = false;
                append_json_string(output, name);
                output.push_back(':');
            }

            void field(std::string_view name, const std::optional<std::string> &value) {
                if (!value) {
                    return;
                }

                key(name);
                append_json_string(output, *value);
            }

            void field(std::string_view name, const std::optional<bool> &value) {
                if (!value) {
                    return;
                }

                key(name);
                output += *value ? "true" : "false";
            }

            void field(std::string_view name, const std::optional<uint32_t> &value) {
                if (!value) {
                    return;
                }

                key(name);
                output += std::to_string(*value);
            }

            void field(std::string_view name, const std::optional<Luau::Location> &value) {
                if (!value) {
                    return;
                }

                key(name);
                append_location(output, *value);
            }

            void field(std::string_view name, const std::optional<std::array<uint32_t, 4>> &value) {
                if (!value) {
                    return;
                }

                key(name);
                append_location(output, *value);
            }

            void field(std::string_view name, const std::optional<std::array<float, 4>> &value) {
                if (!value) {
                    return;
                }

                key(name);
                output.push_back('[');

                for (size_t index = 0; index < value->size(); ++index) {
                    if (index != 0) {
                        output.push_back(',');
                    }

                    output += std::to_string((*value)[index]);
                }

                output.push_back(']');
            }

            void field(std::string_view name, const std::optional<std::vector<std::string>> &value) {
                if (!value) {
                    return;
                }

                key(name);
                output.push_back('[');

                for (size_t index = 0; index < value->size(); ++index) {
                    if (index != 0) {
                        output.push_back(',');
                    }

                    append_json_string(output, (*value)[index]);
                }

                output.push_back(']');
            }

            std::string output;
            bool first = true;
            bool first_field = true;
        };

        std::optional<Luau::ModulePtr> module_for(Luau::Frontend &frontend, std::string_view name) {
            const std::string module_name(name);
            const Luau::ModulePtr module = frontend.moduleResolver.getModule(module_name);

            if (!module) {
                return std::nullopt;
            }

            return module;
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

        std::optional<Luau::TypeId> type_at(
            const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position) {

            if (const std::optional<Luau::AstType *> type_node = type_node_at(source, position)) {
                if (const Luau::TypeId *type = module.astResolvedTypes.find(*type_node)) {
                    return *type;
                }
            }

            Luau::ExprOrLocal expression = Luau::findExprOrLocalAtPosition(source, position);

            if (Luau::AstExpr *expr = expression.getExpr()) {
                if (const Luau::TypeId *type = module.astTypes.find(expr)) {
                    return *type;
                }

                if (const Luau::TypePackId *pack = module.astTypePacks.find(expr)) {
                    return Luau::first(*pack);
                }
            }

            if (Luau::AstLocal *local = expression.getLocal()) {
                if (std::optional<Luau::Binding> binding = Luau::findBindingAtPosition(module, source, position)) {
                    return binding->typeId;
                }

                Luau::ScopePtr scope = Luau::findScopeAtPosition(module, position);

                while (scope) {
                    if (auto binding = scope->bindings.find(Luau::Symbol(local)); binding != scope->bindings.end()) {
                        return binding->second.typeId;
                    }

                    scope = scope->parent;
                }
            }

            return std::nullopt;
        }

        std::optional<std::string> expression_name(Luau::AstExpr *expression) {
            if (auto local = expression->as<Luau::AstExprLocal>()) {
                return local->local->name.value;
            }

            if (auto global = expression->as<Luau::AstExprGlobal>()) {
                return global->name.value;
            }

            if (auto index = expression->as<Luau::AstExprIndexName>()) {
                return index->index.value;
            }

            return std::nullopt;
        }

        std::optional<std::string> target_name(Luau::ExprOrLocal &target) {
            if (Luau::AstExpr *expression = target.getExpr()) {
                if (auto index = expression->as<Luau::AstExprIndexName>()) {
                    if (auto base = expression_name(index->expr)) {
                        return *base + (index->op == ':' ? ":" : ".") + index->index.value;
                    }
                }

                return expression_name(expression);
            }

            if (Luau::AstLocal *local = target.getLocal()) {
                return local->name.value;
            }

            return std::nullopt;
        }

        std::optional<Luau::Location> target_location(Luau::ExprOrLocal &target) {
            if (Luau::AstExpr *expression = target.getExpr()) {
                if (auto index = expression->as<Luau::AstExprIndexName>()) {
                    return index->indexLocation;
                }

                return expression->location;
            }

            if (Luau::AstLocal *local = target.getLocal()) {
                return local->location;
            }

            return std::nullopt;
        }

        std::optional<std::string> documentation_text(
            Luau::Frontend &frontend, const Luau::ModuleName &module_name, Luau::Position position) {

            if (!frontend.fileResolver) {
                return std::nullopt;
            }

            const std::optional<Luau::SourceCode> source = frontend.fileResolver->readSource(module_name);

            if (!source) {
                return std::nullopt;
            }

            std::vector<std::string_view> lines;
            size_t start = 0;

            while (start <= source->source.size()) {
                const size_t end = source->source.find('\n', start);

                lines.push_back(std::string_view(source->source)
                        .substr(start, end == std::string::npos ? std::string::npos : end - start));

                if (end == std::string::npos) {
                    break;
                }

                start = end + 1;
            }

            if (position.line == 0 || position.line > lines.size()) {
                return std::nullopt;
            }

            size_t first = position.line;

            while (first > 0) {
                std::string_view line = lines[first - 1];
                const size_t indentation = line.find_first_not_of(" \t");

                if (indentation == std::string_view::npos) {
                    break;
                }

                line.remove_prefix(indentation);

                if (line.compare(0, 3, "---") != 0) {
                    break;
                }

                --first;
            }

            if (first == position.line) {
                return std::nullopt;
            }

            std::string result;

            for (size_t index = first; index < position.line; ++index) {
                if (!result.empty()) {
                    result.push_back('\n');
                }

                result += lines[index];
            }

            return result;
        }

        std::string type_description(Luau::TypeId type, const std::optional<std::string> &name = std::nullopt,
            const Luau::ScopePtr &scope = nullptr, bool exhaustive = false) {
            Luau::ToStringOptions options;
            options.exhaustive = exhaustive;
            options.functionTypeArguments = true;
            options.hideFunctionSelfArgument = true;
            options.scope = scope;

            type = Luau::follow(type);

            if (const Luau::FunctionType *function = Luau::get<Luau::FunctionType>(type); function && name) {
                return Luau::toStringNamedFunction(*name, *function, options);
            }

            return Luau::toString(type, options);
        }

        const Luau::Property *property_for(Luau::TypeId type, std::string_view name) {
            type = Luau::follow(type);

            if (const Luau::TableType *table = Luau::get<Luau::TableType>(type)) {
                if (table->boundTo && *table->boundTo != type) {
                    if (const Luau::Property *property = property_for(*table->boundTo, name)) {
                        return property;
                    }
                }

                if (auto property = table->props.find(std::string(name)); property != table->props.end()) {
                    return &property->second;
                }
            }

            if (const Luau::ExternType *external = Luau::get<Luau::ExternType>(type)) {
                if (auto property = external->props.find(std::string(name)); property != external->props.end()) {
                    return &property->second;
                }
            }

            if (const Luau::MetatableType *metatable = Luau::get<Luau::MetatableType>(type)) {
                if (const Luau::Property *property = property_for(metatable->table, name)) {
                    return property;
                }

                return property_for(metatable->metatable, name);
            }

            if (const Luau::UnionType *union_type = Luau::get<Luau::UnionType>(type)) {
                for (Luau::TypeId option : union_type->options) {
                    if (const Luau::Property *property = property_for(option, name)) {
                        return property;
                    }
                }
            }

            if (const Luau::IntersectionType *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (Luau::TypeId part : intersection->parts) {
                    if (const Luau::Property *property = property_for(part, name)) {
                        return property;
                    }
                }
            }

            return nullptr;
        }

        std::string definition_module_for(Luau::TypeId type, const Luau::Module &module) {
            type = Luau::follow(type);

            if (const Luau::TableType *table = Luau::get<Luau::TableType>(type)) {
                if (!table->definitionModuleName.empty()) {
                    return table->definitionModuleName;
                }
            }

            if (const Luau::ExternType *external = Luau::get<Luau::ExternType>(type)) {
                if (!external->definitionModuleName.empty()) {
                    return external->definitionModuleName;
                }
            }

            return module.name;
        }

        EditorEntry location_entry(const Luau::ModuleName &module_name, const Luau::Location &location,
            const std::optional<std::string> &name) {
            EditorEntry entry;
            entry.name = name;
            entry.path = module_name;
            entry.range = location;
            entry.selection = location;
            entry.declaration = true;

            return entry;
        }

        std::optional<std::string> type_documentation(Luau::TypeId type) {
            type = Luau::follow(type);
            constexpr std::string_view marker = "/globaltype/";

            if ((*type).documentationSymbol) {
                const std::string &symbol = *(*type).documentationSymbol;
                const size_t marker_position = symbol.rfind(marker);

                if (marker_position != std::string::npos) {
                    return "@roblox/globaltype/" + symbol.substr(marker_position + marker.size());
                }

                return symbol;
            }

            if (const Luau::ExternType *external = Luau::get<Luau::ExternType>(type)) {
                const size_t marker_position = external->name.rfind(marker);

                if (marker_position != std::string::npos) {
                    return "@roblox/globaltype/" + external->name.substr(marker_position + marker.size());
                }

                return "@roblox/globaltype/" + external->name;
            }

            return std::nullopt;
        }

        std::optional<EditorEntry> type_definition_entry(
            const Luau::Module &module, Luau::TypeId type, const std::optional<std::string> &name) {
            type = Luau::follow(type);

            if (const Luau::FunctionType *function = Luau::get<Luau::FunctionType>(type);
                function && function->definition) {
                const Luau::FunctionDefinition &definition = *function->definition;

                Luau::Location location = definition.originalNameLocation.begin.hasValue()
                                              ? definition.originalNameLocation
                                              : definition.definitionLocation;

                if (name && location.begin.line == location.end.line && location.end.column >= location.begin.column) {
                    const uint32_t width = location.end.column - location.begin.column;
                    const uint32_t name_length = uint32_t(name->size());

                    if (width > name_length) {
                        location.begin.column = location.end.column - name_length;
                    }
                }

                const Luau::ModuleName module_name = definition.definitionModuleName.value_or(module.name);

                return location_entry(module_name, location, name);
            }

            if (const Luau::ExternType *external = Luau::get<Luau::ExternType>(type);
                external && external->definitionLocation) {
                return location_entry(external->definitionModuleName, *external->definitionLocation, name);
            }

            if (const Luau::TableType *table = Luau::get<Luau::TableType>(type)) {
                if (table->definitionLocation.begin.hasValue()) {
                    return location_entry(
                        table->definitionModuleName.empty() ? module.name : table->definitionModuleName,
                        table->definitionLocation, name);
                }
            }

            return std::nullopt;
        }

        struct LocationFinder final : Luau::AstVisitor {
            explicit LocationFinder(const Luau::Location &target) : target(target) {}

            bool visit(Luau::AstNode *node) override {
                if (node->location == target) {
                    found = true;
                }

                return !found;
            }

            bool visit(Luau::AstType *node) override { return visit(static_cast<Luau::AstNode *>(node)); }

            bool visit(Luau::AstTypeTable *node) override {
                for (const Luau::AstTableProp &property : node->props) {
                    if (property.location == target) {
                        found = true;
                        break;
                    }
                }

                return visit(static_cast<Luau::AstType *>(node));
            }

            bool visit(Luau::AstTypePack *node) override { return visit(static_cast<Luau::AstNode *>(node)); }

            const Luau::Location &target;
            bool found = false;
        };

        struct MemberFunctionFinder final : Luau::AstVisitor {
            explicit MemberFunctionFinder(std::string_view name) : name(name) {}

            bool visit(Luau::AstStatFunction *statement) override {
                if (auto index = statement->name->as<Luau::AstExprIndexName>();
                    index && std::string_view(index->index.value) == name) {
                    location = index->indexLocation;
                }

                return true;
            }

            std::string_view name;
            std::optional<Luau::Location> location;
        };

        std::optional<EditorEntry> concrete_member_definition(
            Luau::Frontend &frontend, const Luau::Property &property, std::string_view name) {

            if (!property.typeLocation) {
                return std::nullopt;
            }

            for (const auto &[module_name, source] : frontend.sourceModules) {
                if (!source || !source->root) {
                    continue;
                }

                LocationFinder locations(*property.typeLocation);
                source->root->visit(&locations);

                if (!locations.found) {
                    continue;
                }

                MemberFunctionFinder functions(name);
                source->root->visit(&functions);

                if (functions.location) {
                    return location_entry(module_name, *functions.location, std::string(name));
                }
            }

            return std::nullopt;
        }

        std::optional<EditorEntry> property_definition_entry(Luau::Frontend &frontend, const Luau::Module &module,
            Luau::AstExprIndexName *index, Luau::TypeId base_type, bool resolve_concrete = true) {
            const Luau::Property *property = property_for(base_type, index->index.value);

            if (!property) {
                return std::nullopt;
            }

            if (property->readTy) {
                if (std::optional<EditorEntry> definition =
                        type_definition_entry(module, *property->readTy, index->index.value)) {
                    if (definition->range && definition->range->begin == definition->range->end && property->location) {
                        definition->range = *property->location;
                        definition->selection = *property->location;
                    }

                    return definition;
                }
            }

            if (resolve_concrete) {
                if (std::optional<EditorEntry> definition =
                        concrete_member_definition(frontend, *property, index->index.value)) {
                    return definition;
                }
            }

            if (!property->location && !property->typeLocation) {
                return std::nullopt;
            }

            const Luau::Location location = property->location.value_or(*property->typeLocation);

            return location_entry(definition_module_for(base_type, module), location, index->index.value);
        }

        struct TypeAliasFinder final : Luau::AstVisitor {
            explicit TypeAliasFinder(std::string_view name) : name(name) {}

            bool visit(Luau::AstStatTypeAlias *alias) override {
                if (alias->name.value && std::string_view(alias->name.value) == name) {
                    location = alias->nameLocation;
                }

                return true;
            }

            std::string_view name;
            std::optional<Luau::Location> location;
        };

        std::optional<EditorEntry> find_type_alias(Luau::Frontend &frontend, std::string_view name) {
            for (const auto &[module_name, source] : frontend.sourceModules) {
                if (!source || !source->root) {
                    continue;
                }

                TypeAliasFinder finder(name);
                source->root->visit(&finder);

                if (finder.location) {
                    return location_entry(module_name, *finder.location, std::string(name));
                }
            }

            return std::nullopt;
        }

        std::optional<Luau::AstStatTypeAlias *> type_alias_at(
            const Luau::SourceModule &source, Luau::Position position) {

            struct Finder final : Luau::AstVisitor {
                explicit Finder(Luau::Position position) : position(position) {}

                bool visit(Luau::AstStatTypeAlias *alias) override {
                    if (alias->nameLocation.contains(position)) {
                        result = alias;
                    }

                    return true;
                }

                Luau::Position position;
                Luau::AstStatTypeAlias *result = nullptr;
            } finder(position);

            source.root->visit(&finder);

            return finder.result ? std::optional<Luau::AstStatTypeAlias *>(finder.result) : std::nullopt;
        }

        std::string type_alias_description(const Luau::Module &module, Luau::AstStatTypeAlias *alias) {
            std::string result = "type ";
            result += alias->name.value;

            if (alias->generics.size != 0 || alias->genericPacks.size != 0) {
                result.push_back('<');

                bool first = true;

                for (Luau::AstGenericType *generic : alias->generics) {
                    if (!first) {
                        result += ", ";
                    }

                    first = false;
                    result += generic->name.value;

                    if (generic->defaultValue) {
                        result += " = ";

                        if (const Luau::TypeId *type = module.astResolvedTypes.find(generic->defaultValue)) {
                            result += type_description(*type);
                        }
                    }
                }

                for (Luau::AstGenericTypePack *generic : alias->genericPacks) {
                    if (!first) {
                        result += ", ";
                    }

                    first = false;
                    result += generic->name.value;
                }

                result.push_back('>');
            }

            result += " = ";

            if (const Luau::TypeId *type = module.astResolvedTypes.find(alias->type)) {
                result += type_description(*type);
            }

            return result;
        }

        std::optional<Luau::Binding> type_prefix_binding(
            const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position) {
            const std::optional<Luau::AstType *> type = type_node_at(source, position);

            if (auto reference = type ? (*type)->as<Luau::AstTypeReference>() : nullptr;
                reference && reference->prefix && reference->prefixLocation &&
                reference->prefixLocation->contains(position)) {
                Luau::ScopePtr scope = Luau::findScopeAtPosition(module, position);

                while (scope) {
                    if (std::optional<Luau::Binding> binding =
                            scope->linearSearchForBinding(reference->prefix->value, false)) {
                        return binding;
                    }

                    scope = scope->parent;
                }
            }

            return std::nullopt;
        }

        std::optional<EditorEntry> type_prefix_definition(
            const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position) {
            const std::optional<Luau::AstType *> type = type_node_at(source, position);

            if (auto reference = type ? (*type)->as<Luau::AstTypeReference>() : nullptr;
                reference && reference->prefix && reference->prefixLocation &&
                reference->prefixLocation->contains(position)) {
                if (std::optional<Luau::Binding> binding = type_prefix_binding(module, source, position)) {
                    return location_entry(source.name, binding->location, std::string(reference->prefix->value));
                }
            }

            return std::nullopt;
        }

        std::optional<Luau::AstLocal *> target_local(const Luau::SourceModule &source, Luau::Position position) {
            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);

            if (Luau::AstLocal *local = target.getLocal()) {
                return local;
            }

            if (Luau::AstExpr *expression = target.getExpr()) {
                if (auto local_expression = expression->as<Luau::AstExprLocal>()) {
                    return local_expression->local;
                }
            }

            if (const std::optional<Luau::AstType *> type = type_node_at(source, position)) {
                if (auto reference = (*type)->as<Luau::AstTypeReference>();
                    reference && reference->prefixLocal && reference->prefixLocation &&
                    reference->prefixLocation->contains(position)) {
                    return reference->prefixLocal;
                }
            }

            return std::nullopt;
        }

        struct LocalVisitor final : Luau::AstVisitor {
            LocalVisitor(Luau::AstLocal *target, std::string_view module_name)
                : target(target), module_name(module_name) {}

            bool visit(Luau::AstExprLocal *expression) override {
                if (expression->local == target) {
                    add(expression->local->location == expression->location ? expression->location
                                                                            : expression->location,
                        false);
                }

                return true;
            }

            bool visit(Luau::AstStatLocal *statement) override {
                for (Luau::AstLocal *local : statement->vars) {
                    if (local == target) {
                        add(local->location, true);
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *statement) override {
                if (statement->name == target) {
                    add(target->location, true);
                }

                return true;
            }

            bool visit(Luau::AstStatFor *statement) override {
                if (statement->var == target) {
                    add(target->location, true);
                }

                return true;
            }

            bool visit(Luau::AstStatForIn *statement) override {
                for (Luau::AstLocal *local : statement->vars) {
                    if (local == target) {
                        add(local->location, true);
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprFunction *function) override {
                if (function->self == target) {
                    add(target->location, true);
                }

                for (Luau::AstLocal *local : function->args) {
                    if (local == target) {
                        add(local->location, true);
                    }
                }

                return true;
            }

            std::vector<EditorEntry> entries;

          private:
            void add(const Luau::Location &location, bool declaration) {
                EditorEntry entry;
                entry.name = target->name.value;
                entry.path = module_name;
                entry.range = location;
                entry.selection = location;
                entry.kind = 13;
                entry.declaration = declaration;
                entries.push_back(std::move(entry));
            }

            Luau::AstLocal *target;
            std::string module_name;
        };

        void append_local_entries(
            JsonWriter &writer, const Luau::SourceModule &source, Luau::Position position, std::string_view operation) {
            const std::optional<Luau::AstLocal *> local = target_local(source, position);

            if (!local) {
                return;
            }

            LocalVisitor visitor(*local, source.name);
            source.root->visit(&visitor);

            std::sort(
                visitor.entries.begin(), visitor.entries.end(), [](const EditorEntry &left, const EditorEntry &right) {
                    return left.range->begin < right.range->begin;
                });

            if (operation == "definition" || operation == "typeDefinition") {
                for (const EditorEntry &entry : visitor.entries) {
                    if (entry.declaration == std::optional<bool>(true)) {
                        writer.add(entry);
                    }
                }
            } else if (operation == "prepare" || operation == "references" || operation == "localReferences") {
                for (const EditorEntry &entry : visitor.entries) {
                    writer.add(entry);
                }
            } else if (operation == "scope") {
                EditorEntry entry;
                entry.name = (*local)->name.value;
                entry.path = source.name;
                entry.range = (*local)->location;
                entry.selection = (*local)->location;
                entry.kind = 13;
                writer.add(entry);
            }
        }

        std::optional<EditorEntry> local_definition_entry(const Luau::SourceModule &source, Luau::Position position) {
            const std::optional<Luau::AstLocal *> local = target_local(source, position);

            if (!local) {
                return std::nullopt;
            }

            LocalVisitor visitor(*local, source.name);
            source.root->visit(&visitor);

            for (const EditorEntry &entry : visitor.entries) {
                if (entry.declaration == std::optional<bool>(true)) {
                    return entry;
                }
            }

            return std::nullopt;
        }

        struct DefinitionIdentity {
            Luau::ModuleName module;
            Luau::Location location;
        };

        bool same_identity(const DefinitionIdentity &left, const EditorEntry &right) {
            return right.path && right.range && *right.path == left.module && *right.range == left.location;
        }

        std::optional<DefinitionIdentity> identity_for_target(Luau::Frontend &frontend, const Luau::Module &module,
            const Luau::SourceModule &source, Luau::Position position) {

            if (std::optional<EditorEntry> definition = type_prefix_definition(module, source, position);
                definition && definition->path && definition->range) {
                return DefinitionIdentity{*definition->path, *definition->range};
            }

            if (const std::optional<Luau::AstStatTypeAlias *> alias = type_alias_at(source, position)) {
                return DefinitionIdentity{source.name, (*alias)->nameLocation};
            }

            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);

            if (Luau::AstExpr *expression = target.getExpr()) {
                if (auto index = expression->as<Luau::AstExprIndexName>()) {
                    if (const Luau::TypeId *base_type = module.astTypes.find(index->expr)) {
                        if (std::optional<EditorEntry> definition =
                                property_definition_entry(frontend, module, index, *base_type);
                            definition && definition->path && definition->range) {
                            return DefinitionIdentity{*definition->path, *definition->range};
                        }
                    }
                }

                if (const Luau::TypeId *type = module.astTypes.find(expression)) {
                    if (std::optional<EditorEntry> definition =
                            type_definition_entry(module, *type, target_name(target));
                        definition && definition->path && definition->range) {
                        return DefinitionIdentity{*definition->path, *definition->range};
                    }
                }
            }

            Luau::AstNode *node = Luau::findNodeAtPosition(source, position);

            if (auto string = node ? node->as<Luau::AstExprConstantString>() : nullptr;
                string && string->quoteStyle == Luau::AstExprConstantString::QuoteStyle::Unquoted) {
                return DefinitionIdentity{source.name, string->location};
            }

            const std::optional<Luau::AstType *> type = type_node_at(source, position);

            if (auto reference = type ? (*type)->as<Luau::AstTypeReference>() : nullptr) {
                if (std::optional<EditorEntry> definition = find_type_alias(frontend, reference->name.value);
                    definition && definition->path && definition->range) {
                    return DefinitionIdentity{*definition->path, *definition->range};
                }
            }

            return std::nullopt;
        }

        struct ReferenceVisitor final : Luau::AstVisitor {
            ReferenceVisitor(const DefinitionIdentity &target, Luau::Frontend &frontend, const Luau::Module &module,
                const Luau::SourceModule &source)
                : target(target), frontend(frontend), module(module), source(source) {}

            bool visit(Luau::AstExprIndexName *index) override {
                if (const Luau::TypeId *base_type = module.astTypes.find(index->expr)) {
                    if (std::optional<EditorEntry> definition =
                            property_definition_entry(frontend, module, index, *base_type);
                        definition && same_identity(target, *definition)) {
                        add(index->indexLocation, false);
                    }
                }

                return true;
            }

            bool visit(Luau::AstExprGlobal *global) override {
                if (const Luau::TypeId *type = module.astTypes.find(global)) {
                    if (std::optional<EditorEntry> definition =
                            type_definition_entry(module, *type, global->name.value);
                        definition && same_identity(target, *definition)) {
                        add(global->location, false);
                    }
                }

                return true;
            }

            bool visit(Luau::AstTypeReference *reference) override {
                if (std::optional<EditorEntry> definition = find_type_alias(frontend, reference->name.value);
                    definition && same_identity(target, *definition)) {
                    add(reference->location, false);
                }

                return true;
            }

            bool visit(Luau::AstExprTable *table) override {
                for (const Luau::AstExprTable::Item &item : table->items) {
                    auto key = item.key ? item.key->as<Luau::AstExprConstantString>() : nullptr;

                    if (key && key->quoteStyle == Luau::AstExprConstantString::QuoteStyle::Unquoted &&
                        target.module == source.name && key->location == target.location) {
                        add(key->location, true);
                    }
                }

                return true;
            }

            std::vector<EditorEntry> entries;

          private:
            void add(const Luau::Location &location, bool declaration) {
                EditorEntry entry;
                entry.path = source.name;
                entry.range = location;
                entry.selection = location;
                entry.declaration = declaration;
                entries.push_back(std::move(entry));
            }

            const DefinitionIdentity &target;
            Luau::Frontend &frontend;
            const Luau::Module &module;
            const Luau::SourceModule &source;
        };

        void append_prepare(JsonWriter &writer, Luau::Frontend &frontend, const Luau::Module &module,
            const Luau::SourceModule &source, Luau::Position position) {

            if (target_local(source, position)) {
                append_local_entries(writer, source, position, "prepare");

                return;
            }

            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);
            std::optional<Luau::Location> location = target_location(target);

            if (!location) {
                return;
            }

            EditorEntry entry;
            entry.name = target_name(target);
            entry.path = source.name;
            entry.range = *location;
            entry.selection = *location;
            entry.declaration = true;
            writer.add(entry);
        }

        void append_scope(
            JsonWriter &writer, const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position) {

            if (target_local(source, position)) {
                append_local_entries(writer, source, position, "scope");

                return;
            }

            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);

            if (Luau::AstExpr *expression = target.getExpr()) {
                if (auto index = expression->as<Luau::AstExprIndexName>()) {
                    EditorEntry entry;
                    entry.name = index->index.value;
                    entry.path = source.name;
                    entry.range = index->indexLocation;
                    entry.selection = index->indexLocation;
                    entry.kind = 6;
                    writer.add(entry);

                    return;
                }
            }

            const std::optional<Luau::AstType *> type = type_node_at(source, position);

            if (std::optional<Luau::Binding> binding = type_prefix_binding(module, source, position)) {
                auto reference = (*type)->as<Luau::AstTypeReference>();
                EditorEntry entry;
                entry.name = reference->prefix->value;
                entry.path = source.name;
                entry.range = binding->location;
                entry.selection = binding->location;
                entry.kind = 13;
                writer.add(entry);

                return;
            }

            if (auto reference = type ? (*type)->as<Luau::AstTypeReference>() : nullptr) {
                if (const Luau::TypeId *type = module.astResolvedTypes.find(reference)) {
                    Luau::TypeId resolved = Luau::follow(*type);

                    if (const Luau::TableType *table = Luau::get<Luau::TableType>(resolved)) {
                        for (const auto &[name, property] : table->props) {
                            EditorEntry entry;
                            entry.name = name;
                            entry.path = source.name;
                            entry.range = reference->location;
                            entry.selection = reference->location;
                            entry.kind = 8;
                            writer.add(entry);
                        }
                    } else if (const Luau::ExternType *external = Luau::get<Luau::ExternType>(resolved)) {
                        for (const auto &[name, property] : external->props) {
                            EditorEntry entry;
                            entry.name = name;
                            entry.path = source.name;
                            entry.range = reference->location;
                            entry.selection = reference->location;
                            entry.kind = 8;
                            writer.add(entry);
                        }
                    }
                }
            }
        }

        void append_references(JsonWriter &writer, Luau::Frontend &frontend, const Luau::Module &module,
            const Luau::SourceModule &source, Luau::Position position) {

            if (target_local(source, position)) {
                append_local_entries(writer, source, position, "references");

                return;
            }

            const std::optional<DefinitionIdentity> target = identity_for_target(frontend, module, source, position);

            if (!target) {
                return;
            }

            if (std::optional<EditorEntry> definition = location_entry(target->module, target->location, std::nullopt);
                definition) {
                definition->declaration = true;
                writer.add(*definition);
            }

            std::vector<EditorEntry> entries;

            for (const auto &[module_name, source_module] : frontend.sourceModules) {
                if (!source_module || !source_module->root) {
                    continue;
                }

                const Luau::ModulePtr checked_module = frontend.moduleResolver.getModule(module_name);

                if (!checked_module) {
                    continue;
                }

                ReferenceVisitor visitor(*target, frontend, *checked_module, *source_module);
                source_module->root->visit(&visitor);
                entries.insert(entries.end(), visitor.entries.begin(), visitor.entries.end());
            }

            std::sort(entries.begin(), entries.end(), [](const EditorEntry &left, const EditorEntry &right) {
                if (*left.path != *right.path) {
                    return *left.path < *right.path;
                }

                return left.range->begin < right.range->begin;
            });

            for (const EditorEntry &entry : entries) {
                if (entry.path && entry.range && *entry.path == target->module && *entry.range == target->location) {
                    continue;
                }

                writer.add(entry);
            }
        }

        std::optional<EditorEntry> navigation_entry(Luau::Frontend &frontend, const Luau::Module &module,
            const Luau::SourceModule &source, Luau::Position position, std::string_view operation) {

            if (std::optional<EditorEntry> definition = type_prefix_definition(module, source, position)) {
                return definition;
            }

            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);

            if (target_local(source, position)) {
                if (operation == "typeDefinition") {
                    if (const std::optional<Luau::TypeId> type = type_at(module, source, position)) {
                        if (std::optional<EditorEntry> definition =
                                type_definition_entry(module, *type, target_name(target))) {
                            return definition;
                        }
                    }
                }

                return local_definition_entry(source, position);
            }

            if (Luau::AstExpr *expression = target.getExpr()) {
                if (auto string = expression->as<Luau::AstExprConstantString>()) {
                    if (std::optional<Luau::ModuleInfo> info =
                            frontend.moduleResolver.resolveModuleInfo(source.name, *string)) {
                        return location_entry(info->name, Luau::Location(), std::nullopt);
                    }
                }

                if (auto index = expression->as<Luau::AstExprIndexName>()) {
                    if (const Luau::TypeId *base_type = module.astTypes.find(index->expr)) {
                        if (std::optional<EditorEntry> definition = property_definition_entry(
                                frontend, module, index, *base_type, operation != "typeDefinition")) {
                            return definition;
                        }
                    }
                }

                if (const Luau::TypeId *type = module.astTypes.find(expression)) {
                    return type_definition_entry(module, *type, target_name(target));
                }
            }

            const std::optional<Luau::AstType *> type = type_node_at(source, position);

            if (auto reference = type ? (*type)->as<Luau::AstTypeReference>() : nullptr) {
                if (const Luau::TypeId *type = module.astResolvedTypes.find(reference)) {
                    if (std::optional<EditorEntry> definition =
                            type_definition_entry(module, *type, reference->name.value)) {
                        return definition;
                    }
                }

                return find_type_alias(frontend, reference->name.value);
            }

            return std::nullopt;
        }

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

        std::string type_pack_description(Luau::TypePackId pack) {
            Luau::ToStringOptions options;
            options.functionTypeArguments = true;
            options.hideFunctionSelfArgument = true;

            return Luau::toString(pack, options);
        }

        struct HintVisitor final : Luau::AstVisitor {
            HintVisitor(const Luau::Module &module, const Luau::SourceModule &source)
                : module(module), source(source) {}

            bool visit(Luau::AstStatLocal *statement) override {
                for (size_t index = 0; index != statement->vars.size && index != statement->values.size; ++index) {
                    Luau::AstLocal *local = statement->vars.data[index];

                    if (local->annotation && local->annotation->location != Luau::Location()) {
                        continue;
                    }

                    std::optional<Luau::TypeId> type;

                    if (const Luau::TypeId *value_type = module.astTypes.find(statement->values.data[index])) {
                        type = *value_type;
                    } else if (const Luau::TypePackId *pack = module.astTypePacks.find(statement->values.data[index])) {
                        type = Luau::first(*pack);
                    } else {
                        type = type_at(module, source, local->location.begin);
                    }

                    if (type) {
                        add(local->location.end, 1, type_description(*type));
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatFunction *statement) override {
                add_return(statement->func);

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *statement) override {
                add_return(statement->func);

                return true;
            }

            bool visit(Luau::AstExprCall *call) override {
                const std::optional<Luau::TypeId> type = call_type(module, call);

                if (!type) {
                    return true;
                }

                const Luau::FunctionType *function = Luau::get<Luau::FunctionType>(Luau::follow(*type));

                if (!function) {
                    return true;
                }

                const auto [parameters, tail] = Luau::flatten(function->argTypes);
                const size_t self_offset = function->hasSelf && !parameters.empty() ? 1 : 0;

                for (size_t index = 0; index != call->args.size; ++index) {
                    const size_t parameter = index + self_offset;

                    if (parameter >= function->argNames.size() || !function->argNames[parameter]) {
                        continue;
                    }

                    const bool literal = call->args.data[index]->is<Luau::AstExprConstantString>() ||
                                         call->args.data[index]->is<Luau::AstExprConstantNumber>() ||
                                         call->args.data[index]->is<Luau::AstExprConstantInteger>() ||
                                         call->args.data[index]->is<Luau::AstExprConstantBool>();

                    add(Luau::Location(call->args.data[index]->location.begin, call->args.data[index]->location.begin),
                        literal ? 4 : 5, function->argNames[parameter]->name);
                }

                return true;
            }

            std::vector<EditorEntry> entries;

          private:
            void add_return(Luau::AstExprFunction *function) {
                if (function->returnAnnotation && function->returnAnnotation->location != Luau::Location()) {
                    return;
                }

                if (Luau::ScopePtr scope = Luau::findScopeAtPosition(module, function->body->location.begin)) {
                    add(function->location.end, 3, type_pack_description(scope->returnType));

                    return;
                }

                const Luau::TypeId *type = module.astTypes.find(function);

                if (type) {
                    if (const Luau::FunctionType *value = Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                        add(function->location.end, 3, type_pack_description(value->retTypes));

                        return;
                    }
                }

                struct ReturnVisitor final : Luau::AstVisitor {
                    ReturnVisitor(const Luau::Module &module, HintVisitor &owner) : module(module), owner(owner) {}

                    bool visit(Luau::AstStatReturn *statement) override {
                        if (statement->list.size != 0) {
                            if (const Luau::TypeId *type = module.astTypes.find(statement->list.data[0])) {
                                owner.add(owner.source.root->location, 3, type_description(*type));
                                found = true;
                            }
                        }

                        return !found;
                    }

                    const Luau::Module &module;
                    HintVisitor &owner;
                    bool found = false;
                } visitor(module, *this);

                function->body->visit(&visitor);

                if (visitor.found) {
                    entries.back().range = Luau::Location(function->location.end, function->location.end);
                    entries.back().selection = entries.back().range;
                }
            }

            void add(const Luau::Position &position, uint32_t kind, std::string value) {
                add(Luau::Location(position, position), kind, std::move(value));
            }

            void add(const Luau::Location &location, uint32_t kind, std::string value) {
                EditorEntry entry;
                entry.path = source.name;
                entry.range = Luau::Location(location.end, location.end);
                entry.selection = entry.range;
                entry.kind = kind;
                entry.description = std::move(value);
                entries.push_back(std::move(entry));
            }

            void add(const Luau::Location &location, uint32_t kind, std::string_view value) {
                add(location, kind, std::string(value));
            }

            const Luau::Module &module;
            const Luau::SourceModule &source;
        };

        void append_hints(JsonWriter &writer, const Luau::Module &module, const Luau::SourceModule &source) {
            HintVisitor visitor(module, source);
            source.root->visit(&visitor);

            std::sort(
                visitor.entries.begin(), visitor.entries.end(), [](const EditorEntry &left, const EditorEntry &right) {
                    return left.range->begin < right.range->begin;
                });

            for (const EditorEntry &entry : visitor.entries) {
                writer.add(entry);
            }
        }

        std::vector<std::string> function_parameters(const Luau::FunctionType &function) {
            const auto [parameters, tail] = Luau::flatten(function.argTypes);
            std::vector<std::string> result;
            const size_t self_offset = function.hasSelf && !parameters.empty() ? 1 : 0;

            for (size_t index = self_offset; index < parameters.size(); ++index) {
                std::string parameter;

                if (index < function.argNames.size() && function.argNames[index]) {
                    parameter += function.argNames[index]->name;
                    parameter += ": ";
                }

                parameter += type_description(parameters[index]);
                result.push_back(std::move(parameter));
            }

            if (tail) {
                result.push_back("...");
            }

            return result;
        }

        struct TokenVisitor final : Luau::AstVisitor {
            TokenVisitor(Luau::Frontend &frontend, const Luau::SourceModule &source)
                : frontend(frontend), source(source) {}

            bool visit(Luau::AstExprGlobal *expression) override {
                add(expression->location, 12, false, 0, expression->name.value);

                return true;
            }

            bool visit(Luau::AstExprLocal *expression) override {
                add(expression->location, 28, false, 0, expression->local->name.value);

                return true;
            }

            bool visit(Luau::AstExprIndexName *expression) override {
                add(expression->indexLocation, 6, false, 0, expression->index.value);

                return true;
            }

            bool visit(Luau::AstExprConstantNumber *expression) override {
                add(expression->location, 7, false);

                return true;
            }

            bool visit(Luau::AstExprConstantInteger *expression) override {
                add(expression->location, 7, false);

                return true;
            }

            bool visit(Luau::AstExprConstantBool *expression) override {
                add(expression->location, 7, false);

                return true;
            }

            bool visit(Luau::AstStatLocal *statement) override {
                for (Luau::AstLocal *local : statement->vars) {
                    add(local->location, 28, true, 0, local->name.value);
                }

                if (statement->isConst) {
                    for (Luau::AstLocal *local : statement->vars) {
                        add(local->location, 28, true, 2, local->name.value);
                    }
                }

                return true;
            }

            bool visit(Luau::AstStatLocalFunction *statement) override {
                add(statement->name->location, 12, true, 0, statement->name->name.value);

                return true;
            }

            bool visit(Luau::AstExprFunction *function) override {
                if (function->self) {
                    add(function->self->location, 27, true, 0, function->self->name.value);
                }

                for (Luau::AstLocal *local : function->args) {
                    add(local->location, 27, true, 0, local->name.value);
                }

                return true;
            }

            bool visit(Luau::AstTypeReference *reference) override {
                add(reference->location, 26, false, 0, reference->name.value);

                return true;
            }

            bool visit(Luau::AstExprCall *call) override {
                const auto function = call->func->as<Luau::AstExprGlobal>();

                if (function && function->name == "require" && call->args.size > 0) {
                    Luau::AstExpr *argument = call->args.data[0];

                    while (auto group = argument->as<Luau::AstExprGroup>()) {
                        argument = group->expr;
                    }

                    if (argument->as<Luau::AstExprConstantString>() &&
                        frontend.moduleResolver.resolveModuleInfo(source.name, *argument)) {
                        add(argument->location, 3, false);
                    }
                }

                return true;
            }

          private:
            void add(const Luau::Location &location, uint32_t kind, bool declaration, uint32_t modifiers = 0,
                const std::optional<std::string> &name = std::nullopt) {
                EditorEntry entry;
                entry.name = name;
                entry.path = source.name;
                entry.range = location;
                entry.selection = location;
                entry.kind = kind;
                entry.modifiers = modifiers;
                entry.declaration = declaration;
                entries.push_back(std::move(entry));
            }

            Luau::Frontend &frontend;
            const Luau::SourceModule &source;

          public:
            std::vector<EditorEntry> entries;
        };

        void append_tokens(JsonWriter &writer, Luau::Frontend &frontend, const Luau::SourceModule &source) {
            TokenVisitor visitor(frontend, source);
            source.root->visit(&visitor);

            std::sort(
                visitor.entries.begin(), visitor.entries.end(), [](const EditorEntry &left, const EditorEntry &right) {
                    return left.range->begin < right.range->begin;
                });

            for (const EditorEntry &entry : visitor.entries) {
                writer.add(entry);
            }
        }

        std::optional<Luau::Location> require_location(const Luau::SourceModule &source, Luau::Position position) {
            struct Finder final : Luau::AstVisitor {
                explicit Finder(Luau::Position position) : position(position) {}

                bool visit(Luau::AstExprCall *call) override {
                    auto function = call->func->as<Luau::AstExprGlobal>();

                    if (!function || function->name != "require" || call->args.size == 0) {
                        return true;
                    }

                    Luau::AstExpr *argument = call->args.data[0];

                    while (auto group = argument->as<Luau::AstExprGroup>()) {
                        argument = group->expr;
                    }

                    if (argument->location.contains(position) &&
                        (!result || result->location.encloses(argument->location))) {
                        result = argument;
                    }

                    return true;
                }

                Luau::Position position;
                Luau::AstExpr *result = nullptr;
            } finder(position);

            source.root->visit(&finder);

            return finder.result ? std::optional<Luau::Location>(finder.result->location) : std::nullopt;
        }

        void append_completion(
            JsonWriter &writer, Luau::Frontend &frontend, const Luau::SourceModule &source, Luau::Position position) {

            const Luau::AutocompleteResult result = Luau::autocomplete(frontend, source.name, position,
                [](std::string, std::optional<const Luau::ExternType *>, std::optional<std::string>) {
                    return std::optional<Luau::AutocompleteEntryMap>();
                });

            const std::optional<Luau::Location> require_range = require_location(source, position);

            if (require_range) {
                EditorEntry entry;
                entry.path = source.name;
                entry.range = require_range;
                entry.selection = require_range;
                entry.require = true;
                writer.add(entry);
            }

            for (const auto &[name, candidate] : result.entryMap) {
                EditorEntry entry;
                entry.name = name;
                entry.path = source.name;
                entry.deprecated = candidate.deprecated;
                entry.documentation = candidate.documentationSymbol;
                entry.insert = candidate.insertText;

                if (candidate.kind == Luau::AutocompleteEntryKind::RequirePath) {
                    entry.require = true;
                    entry.range = require_range;
                    entry.selection = require_range;
                }

                if (candidate.type) {
                    entry.description = type_description(*candidate.type, name);
                }

                writer.add(entry);
            }

            if (result.context == Luau::AutocompleteContext::Expression ||
                result.context == Luau::AutocompleteContext::Statement) {
                EditorEntry entry;
                entry.path = source.name;
                entry.range = Luau::Location(position, position);
                entry.imports = true;
                writer.add(entry);
            }
        }

        void append_signature(
            JsonWriter &writer, const Luau::Module &module, const Luau::SourceModule &source, Luau::Position position) {
            CallFinder finder(position);
            source.root->visit(&finder);

            if (!finder.result) {
                return;
            }

            const std::optional<Luau::TypeId> type = call_type(module, finder.result);

            if (!type) {
                return;
            }

            const Luau::FunctionType *function = Luau::get<Luau::FunctionType>(Luau::follow(*type));

            if (!function) {
                return;
            }

            EditorEntry entry;
            entry.path = source.name;
            entry.range = finder.result->location;
            entry.label = type_description(*type, expression_name(finder.result->func));
            entry.parameters = function_parameters(*function);
            entry.documentation = Luau::getDocumentationSymbolAtPosition(source, module, position);

            uint32_t active = 0;

            for (Luau::AstExpr *argument : finder.result->args) {
                if (argument->location.begin >= position) {
                    break;
                }

                ++active;
            }

            entry.active = active;
            writer.add(entry);
        }

        void append_hover(Luau::Frontend &frontend, JsonWriter &writer, const Luau::Module &module,
            const Luau::SourceModule &source, Luau::Position position) {

            if (const std::optional<Luau::AstStatTypeAlias *> alias = type_alias_at(source, position)) {
                EditorEntry entry;
                entry.name = (*alias)->name.value;
                entry.description = type_alias_description(module, *alias);
                entry.path = source.name;
                entry.range = (*alias)->nameLocation;
                entry.selection = entry.range;
                entry.documentation = Luau::getDocumentationSymbolAtPosition(source, module, position);
                entry.documentation_text = documentation_text(frontend, source.name, entry.range->begin);
                writer.add(entry);

                return;
            }

            if (const std::optional<Luau::AstType *> type_node = type_node_at(source, position)) {
                const Luau::TypeId *type = module.astResolvedTypes.find(*type_node);

                if (type) {
                    EditorEntry entry;

                    entry.name =
                        (*type_node)->as<Luau::AstTypeReference>()
                            ? std::optional<std::string>((*type_node)->as<Luau::AstTypeReference>()->name.value)
                            : std::nullopt;

                    entry.description = type_description(*type, entry.name);
                    entry.path = source.name;
                    entry.range = (*type_node)->location;
                    entry.selection = entry.range;
                    entry.documentation = type_documentation(*type);

                    if (std::optional<EditorEntry> definition = type_definition_entry(module, *type, entry.name);
                        definition && definition->path && definition->range) {
                        entry.documentation_text =
                            documentation_text(frontend, *definition->path, definition->range->begin);
                    }

                    writer.add(entry);

                    return;
                }
            }

            Luau::ExprOrLocal target = Luau::findExprOrLocalAtPosition(source, position);
            const std::optional<Luau::TypeId> type = type_at(module, source, position);

            if (!type) {
                return;
            }

            EditorEntry entry;
            entry.name = target_name(target);
            entry.description = type_description(*type, entry.name, module.getModuleScope());

            if (const std::optional<Luau::AstLocal *> local = target_local(source, position); local && entry.name) {
                if (Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                    entry.description = "function " + type_description(*type, entry.name, module.getModuleScope());
                } else {
                    entry.description = std::string((*local)->isConst ? "const " : "local ") + *entry.name + ": " +
                                        type_description(*type, std::nullopt, module.getModuleScope(), true);
                }
            } else if (entry.name && Luau::get<Luau::FunctionType>(Luau::follow(*type))) {
                entry.description = "function " + *entry.description;
            }

            entry.path = source.name;
            entry.range = target_location(target);
            entry.selection = entry.range;
            entry.documentation = type_documentation(*type);

            if (!entry.documentation) {
                entry.documentation = Luau::getDocumentationSymbolAtPosition(source, module, position);
            }

            if (std::optional<EditorEntry> definition = type_definition_entry(module, *type, entry.name);
                definition && definition->path && definition->range) {
                entry.documentation_text = documentation_text(frontend, *definition->path, definition->range->begin);
            } else {
                entry.documentation_text = documentation_text(frontend, source.name, entry.range->begin);
            }

            writer.add(entry);
        }

    } // namespace

    std::string editor_query(Luau::Frontend &frontend, std::string_view module_name, uint32_t line, uint32_t column,
        std::string_view operation) {
        const std::string name(module_name);
        const Luau::SourceModule *source = frontend.getSourceModule(name);
        const std::optional<Luau::ModulePtr> module = module_for(frontend, module_name);

        if (!source || !module || !source->root) {
            return "[]";
        }

        const Luau::Position position(line, column);
        JsonWriter writer;

        if (operation == "hover") {
            append_hover(frontend, writer, **module, *source, position);
        } else if (operation == "tokens") {
            append_tokens(writer, frontend, *source);
        } else if (operation == "completion" || operation == "completionResolve") {
            append_completion(writer, frontend, *source, position);
        } else if (operation == "hints") {
            append_hints(writer, **module, *source);
        } else if (operation == "signature") {
            append_signature(writer, **module, *source, position);
        } else if (operation == "definition" || operation == "declaration" || operation == "typeDefinition") {
            if (std::optional<EditorEntry> entry = navigation_entry(frontend, **module, *source, position, operation)) {
                writer.add(*entry);
            }
        } else if (operation == "references") {
            append_references(writer, frontend, **module, *source, position);
        } else if (operation == "prepare") {
            append_prepare(writer, frontend, **module, *source, position);
        } else if (operation == "localReferences") {
            append_local_entries(writer, *source, position, operation);
        } else if (operation == "scope") {
            append_scope(writer, **module, *source, position);
        }

        return writer.finish();
    }

} // namespace instar
