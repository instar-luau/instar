#ifndef INSTAR_LUAU_NATIVE_CONFIGURATION
#define INSTAR_LUAU_NATIVE_CONFIGURATION

#include "rust/cxx.h"

namespace instar::luau {
    struct Language;

    Language configure(rust::Slice<const Language> previous, rust::Str name, rust::Str source, bool executable);
}

#endif
