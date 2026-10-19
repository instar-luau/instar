#ifndef INSTAR_LUAU_NATIVE_ENUMERATIONS
#define INSTAR_LUAU_NATIVE_ENUMERATIONS

#include "Luau/GlobalTypes.h"
#include "rust/cxx.h"

namespace instar::luau {
    struct Enumeration;

    void construct_enumerations(Luau::GlobalTypes &globals, rust::Slice<const Enumeration> enumerations);
}

#endif
