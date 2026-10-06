#ifndef INSTAR_FRONTEND_ROBLOX_HEADER
#define INSTAR_FRONTEND_ROBLOX_HEADER

#include "rust/cxx.h"

namespace Luau {
    struct GlobalTypes;
}

namespace instar {
    struct NativeClass;
    void register_roblox_magic(Luau::GlobalTypes &globals, rust::Slice<const NativeClass> classes);
} // namespace instar

#endif
