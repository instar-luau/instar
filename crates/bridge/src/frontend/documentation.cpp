#include "instar-bridge/src/boundary.rs.h"

#include "Luau/Type.h"
#include "Luau/TypeUtils.h"
#include "documentation.hpp"
#include "state.hpp"

namespace instar {
    namespace {
        void rebase(std::optional<std::string> &symbol, const std::string &prefix, const std::string &replacement) {
            if (symbol && symbol->compare(0, prefix.size(), prefix) == 0) {
                symbol = replacement + symbol->substr(prefix.size());
            }
        }

        void rebase_type(Luau::TypeId type, const std::string &prefix, const std::string &replacement) {
            type = Luau::follow(type);
            rebase(Luau::asMutable(type)->documentationSymbol, prefix, replacement);

            if (auto *table = Luau::getMutable<Luau::TableType>(type)) {
                for (auto &[name, property] : table->props) {
                    rebase(property.documentationSymbol, prefix, replacement);
                }
            } else if (auto *external = Luau::getMutable<Luau::ExternType>(type)) {
                for (auto &[name, property] : external->props) {
                    rebase(property.documentationSymbol, prefix, replacement);
                }
            }
        }

        const Luau::Property *property(Luau::TypeId type, const std::string &name) {
            type = Luau::follow(type);

            if (const auto *table = Luau::get<Luau::TableType>(type)) {
                const auto found = table->props.find(name);

                return found == table->props.end() ? nullptr : &found->second;
            }

            while (const auto *external = Luau::get<Luau::ExternType>(type)) {
                const auto found = external->props.find(name);

                if (found != external->props.end()) {
                    return &found->second;
                }

                if (!external->parent) {
                    break;
                }

                type = Luau::follow(*external->parent);
            }

            return nullptr;
        }
    } // namespace

    void
    assign_documentation(const Luau::ScopePtr &scope, const std::string &identity, const std::string &namespace_name) {
        const std::string prefix = identity + "/";
        const std::string replacement = namespace_name + "/";

        for (auto &[name, binding] : scope->bindings) {
            rebase(binding.documentationSymbol, prefix, replacement);
            rebase_type(binding.typeId, prefix, replacement);
        }

        for (auto &[name, type] : scope->exportedTypeBindings) {
            rebase_type(type.type, prefix, replacement);
        }
    }

    rust::String NativeFrontend::documentation(rust::Str module, rust::Str symbol) const {
        const auto environment = state->resolver.environments.find(std::string(module));

        const auto scope = environment == state->resolver.environments.end()
                               ? state->frontend.globals.globalScope
                               : state->frontend.getEnvironmentScope(environment->second);

        const std::string path(symbol);
        const auto slash = path.find('/');

        if (slash == std::string::npos) {
            return {};
        }

        const auto dot = path.find('.', slash + 1);
        const std::string root = path.substr(slash + 1, dot == std::string::npos ? std::string::npos : dot - slash - 1);
        Luau::TypeId type = nullptr;
        std::optional<std::string> documentation;

        if (path.substr(0, slash) == "global") {
            const auto binding = scope->lookupEx(state->frontend.globals.globalNames.names->getOrAdd(root.c_str()));

            if (!binding) {
                return {};
            }

            type = binding->first->typeId;
            documentation = binding->first->documentationSymbol;
        } else if (path.substr(0, slash) == "globaltype") {
            const auto found = scope->lookupType(root);

            if (!found) {
                return {};
            }

            type = found->type;
            documentation = Luau::follow(type)->documentationSymbol;
        } else {
            return {};
        }

        auto cursor = dot;

        while (cursor != std::string::npos) {
            const auto next = path.find('.', cursor + 1);

            const auto *member = property(
                type,
                path.substr(cursor + 1, next == std::string::npos ? std::string::npos : next - cursor - 1)
            );

            if (!member) {
                return {};
            }

            documentation = member->documentationSymbol;
            const auto readable = member->readTy;
            const auto writable = member->writeTy;

            if (!readable && !writable) {
                return {};
            }

            type = readable ? *readable : *writable;
            cursor = next;
        }

        return documentation ? rust::String(*documentation) : rust::String{};
    }
} // namespace instar
