#ifndef INSTAR_FRONTEND_NAVIGATION_HEADER
#define INSTAR_FRONTEND_NAVIGATION_HEADER

#include "Luau/TypeFwd.h"
#include "rust/cxx.h"

#include <cstdint>
#include <optional>
#include <string>

namespace Luau {
    class AstStatBlock;
    struct Module;
    struct Position;
    struct SourceModule;
}

namespace instar {
    struct Host;
    struct NativeLocation;
}

namespace instar::frontend {
    struct State;

    void register_declarations(
        State &state, const Host &host, Luau::AstStatBlock &root, const std::string &module,
        const std::string &environment
    );

    rust::Vec<NativeLocation>
    definition(State &state, const Host &host, rust::Str module, uint32_t line, uint32_t column, bool type_definition);

    std::optional<Luau::TypeId>
    inferred_type(const Luau::Module &module, const Luau::SourceModule &source, const Luau::Position &position);

    void property(const Host &host, Luau::TypeId type, const std::string &name, rust::Vec<NativeLocation> &result);
}

#endif
