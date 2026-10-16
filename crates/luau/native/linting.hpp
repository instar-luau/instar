#ifndef INSTAR_LUAU_NATIVE_LINTING
#define INSTAR_LUAU_NATIVE_LINTING

#include "Luau/FileResolver.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "rust/cxx.h"

namespace instar::luau {
    struct Warning;

    rust::Vec<Warning> lint(
        const Luau::SourceModule &source, const Luau::Module &module, Luau::FileResolver &resolver,
        const Luau::ScopePtr &globals
    );
}

#endif
