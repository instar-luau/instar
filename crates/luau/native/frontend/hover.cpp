#include "diagnostics.hpp"
#include "hover.hpp"
#include "navigation.hpp"
#include "state.hpp"

#include "Luau/AstQuery.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"
#include "instar-bridge/src/boundary.rs.h"

#include <stdexcept>
#include <string>

namespace instar::frontend {
    NativeHover hover(State &state, const Host &host, rust::Str module, uint32_t line, uint32_t column) {
        const std::string name(module.data(), module.size());
        const auto *source = state.frontend.getSourceModule(name);
        const auto checked = state.frontend.moduleResolver.getModule(name);

        if (!source || !source->root || !checked || checked->cancelled || checked->timeout) {
            throw std::runtime_error("semantic module is unavailable");
        }

        const Luau::Position position(line, column);
        const auto type = inferred_type(*checked, *source, position);
        auto expression = Luau::findExprOrLocalAtPosition(*source, position);
        const auto range = expression.getLocation();

        if (!type || !range) {
            return {};
        }

        Luau::ToStringOptions options;
        options.functionTypeArguments = true;
        options.scope = Luau::findScopeAtPosition(*checked, position);

        const auto documentation = expression.getLocal()
                                       ? Luau::follow(*type)->documentationSymbol
                                       : Luau::getDocumentationSymbolAtPosition(*source, *checked, position);

        const auto binding = Luau::findBindingAtPosition(*checked, *source, position);
        const auto *function = Luau::get<Luau::FunctionType>(Luau::follow(*type));
        const bool deprecated = (binding && binding->deprecated) || (function && function->isDeprecatedFunction);

        return NativeHover{true,
            location(host, name, *range),
            rust::String(Luau::toString(*type, options)),
            rust::String(documentation.value_or("")),
            deprecated};
    }
}
