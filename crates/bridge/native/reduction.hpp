#ifndef INSTAR_LUAU_NATIVE_REDUCTION
#define INSTAR_LUAU_NATIVE_REDUCTION

#include "Luau/Type.h"

#include <vector>

namespace instar::luau {
    struct PresentationBudget;
    struct PresentationLimits;

    std::vector<Luau::TypeId> reduce_union(
        const Luau::UnionType &value, PresentationBudget &budget, Luau::BuiltinTypes &builtin_types,
        const PresentationLimits &limits
    );
}

#endif
