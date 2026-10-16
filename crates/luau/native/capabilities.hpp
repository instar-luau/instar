#ifndef INSTAR_LUAU_NATIVE_CAPABILITIES
#define INSTAR_LUAU_NATIVE_CAPABILITIES

#include "Luau/GlobalTypes.h"
#include "rust/cxx.h"

namespace instar::luau {
    struct Class;

    void apply(Luau::GlobalTypes &globals, rust::Slice<const Class> classes);
}

#endif
