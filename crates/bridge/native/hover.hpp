#ifndef INSTAR_LUAU_NATIVE_HOVER
#define INSTAR_LUAU_NATIVE_HOVER

#include "query.hpp"

namespace instar::luau::query {
    rust::Vec<NativeHover> hover(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget
    );
}

#endif
