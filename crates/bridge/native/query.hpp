#ifndef INSTAR_LUAU_NATIVE_QUERY
#define INSTAR_LUAU_NATIVE_QUERY

#include "Luau/RequireTracer.h"
#include "host.hpp"
#include "index.hpp"
#include "types.hpp"

#include <string>
#include <vector>

namespace instar::luau::query {
    struct Prepared {
        const Luau::Module &module;
        const Luau::SourceModule &source;
        const std::string &text;
    };

    std::vector<Luau::TypeId> overloads(Luau::TypeId root, PresentationBudget &budget);

    rust::Vec<NativeCompletion> completion(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget, std::vector<CompletionType> &types, std::size_t &candidate_index,
        std::optional<std::size_t> selected
    );

    rust::Vec<NativeSignature> signatures(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        PresentationBudget &budget
    );

    rust::Vec<SourceLocation> definitions(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, Luau::Position position,
        bool should_resolve_types
    );

    Entry target(Luau::Frontend &frontend, const Prepared &prepared, Luau::Position position);

    rust::Vec<Occurrence> rename(
        Luau::Frontend &frontend, const Host &host, const Prepared &prepared, const std::string &name,
        const std::vector<Entry> &targets, std::vector<bool> &declared
    );

    rust::Vec<NativeHint> hints(
        Luau::Frontend &frontend, const Host &host, const Luau::Module &module, const Luau::SourceModule &source,
        PresentationBudget &budget, bool is_expanded, Luau::Position start, Luau::Position end,
        std::optional<HintKind> selected = std::nullopt
    );

    rust::Vec<Import> imports(const Host &host, const std::string &name, const Luau::RequireTraceResult &trace);

    SourceLocation locate(const Host &host, const std::string &identity, const Luau::Location &location);
    std::size_t offset(const std::string &source, Luau::Position position);

    Documentation documentation(
        const Host &host, Luau::TypeId type, const std::string &symbol, const std::string &signature = {},
        rust::Vec<SourceLocation> declarations = {}
    );

    std::string documentation(Luau::TypeId type, PresentationBudget &budget);

    std::string documentation(
        const Luau::SourceModule &source, const Luau::Module &module, Luau::Position position,
        PresentationBudget &budget
    );

    std::string documentation(
        const Luau::SourceModule &source, const Luau::Module &module, const Luau::AstExpr *expression,
        PresentationBudget &budget
    );

    Occurrence occurrence(const Host &host, const std::string &identity, const Entry &entry);
    void validate_identifier(const std::string &name);
}

#endif
