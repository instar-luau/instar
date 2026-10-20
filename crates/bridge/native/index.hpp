#ifndef INSTAR_LUAU_NATIVE_INDEX
#define INSTAR_LUAU_NATIVE_INDEX

#include "Luau/Frontend.h"
#include "Luau/Module.h"
#include "instar-bridge/src/bridge.rs.h"

#include <cstdint>
#include <optional>
#include <string>
#include <unordered_set>
#include <vector>

namespace instar::luau {
    struct Declaration {
        std::string identity;
        Luau::Location location;
    };

    enum class BindingKind : std::uint8_t { Local, Type, Property, Global };

    struct Entry {
        Luau::Location location;
        Declaration declaration;
        OccurrenceRole role;
        bool is_type_namespace = false;
        BindingKind kind;
        std::string name;
        bool is_editable = true;
    };

    struct Index {
        Index(Luau::Frontend &frontend, const Luau::Module &module, const Luau::SourceModule &source);

        std::vector<Entry> entries;
        std::unordered_set<std::string> names;
        std::unordered_set<std::string> properties;
    };

    Index index(Luau::Frontend &frontend, const Luau::Module &module, const Luau::SourceModule &source);
    const Entry *at(const Index &index, Luau::Position position);
    bool is_same_declaration(const Declaration &left, const Declaration &right);
    std::optional<Declaration> definition(Luau::TypeId type, unsigned depth = 0);
    Luau::TypeId local_type(const Luau::Module &module, Luau::AstLocal *local);

    std::optional<Declaration>
    annotation_declaration(Luau::Frontend &frontend, const Luau::Module &module, Luau::AstType *annotation);
}

#endif
