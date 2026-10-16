#ifndef INSTAR_LUAU_NATIVE_DIAGNOSTIC
#define INSTAR_LUAU_NATIVE_DIAGNOSTIC

#include "Luau/Error.h"
#include "Luau/FileResolver.h"

namespace Luau {
    struct Module;
}

namespace instar::luau {
    struct Diagnostic;

    void attribute(Luau::Module &module);
    Diagnostic diagnose(const Luau::TypeError &error, Luau::FileResolver &resolver);
}

#endif
