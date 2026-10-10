#ifndef INSTAR_FRONTEND_REFERENCES_HEADER
#define INSTAR_FRONTEND_REFERENCES_HEADER

#include "rust/cxx.h"

namespace instar {
    struct Host;
    struct NativeReference;
}

namespace instar::frontend {
    struct State;

    rust::Vec<NativeReference> references(State &state, const Host &host, rust::Slice<const rust::String> modules);
}

#endif
