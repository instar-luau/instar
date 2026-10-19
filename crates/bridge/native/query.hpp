#ifndef INSTAR_LUAU_NATIVE_QUERY
#define INSTAR_LUAU_NATIVE_QUERY

#include "Luau/RequireTracer.h"
#include "host.hpp"
#include "index.hpp"

#include <string>
#include <vector>

namespace instar::luau::query {
    struct Prepared {
        const Luau::Module &module;
        const Luau::SourceModule &source;
        const std::string &text;
    };

    Occurrence occurrence(const Host &host, const std::string &identity, const Entry &entry);
    void identifier(const std::string &name);

    rust::Vec<Hover>
    hover(Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position);

    rust::Vec<Completion>
    completion(Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position);

    rust::Vec<Signature> signatures(const Host &host, const Prepared &prepared, Luau::Position position);

    rust::Vec<SourceLocation> definitions(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position, bool types
    );

    Entry target(Luau::Frontend &frontend, const Prepared &prepared, Luau::Position position);

    rust::Vec<Occurrence> rename(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, const std::string &name,
        const std::vector<Entry> &targets, std::vector<bool> &declared
    );

    rust::Vec<Hint> hints(const Host &host, const Luau::Module &module, const Luau::SourceModule &source);
    rust::Vec<Import> imports(const Host &host, const std::string &name, const Luau::RequireTraceResult &trace);
}

#endif
