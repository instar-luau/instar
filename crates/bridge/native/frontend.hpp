#ifndef INSTAR_LUAU_NATIVE_FRONTEND
#define INSTAR_LUAU_NATIVE_FRONTEND

#include "Luau/Frontend.h"
#include "host.hpp"
#include "rust/cxx.h"

#include <cstddef>
#include <cstdint>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <tuple>
#include <unordered_map>
#include <vector>

namespace instar::luau {
    struct Diagnostic;
    struct Flag;
    struct NativeHover;
    struct NativeCompletion;
    struct NativeSignature;
    struct SourceLocation;
    struct Occurrence;
    struct Entry;
    struct NativeHint;
    struct HintSelector;
    struct Position;
    struct Import;

    class Frontend {
    public:
        Frontend(const Context &context, rust::Slice<const Flag> settings, const Luau::FrontendOptions &options);
        ~Frontend();

        rust::Vec<Diagnostic> check(rust::Str module);
        void queue(rust::Str module);
        std::size_t checks() const;
        void invalidate(rust::Str module);
        void refresh();
        void clear();
        void environment(const std::string &name);
        const Instances *hierarchy(const std::string &name);
        rust::Vec<NativeHover> hover(rust::Str module, std::uint32_t line, std::uint32_t column);
        rust::Vec<NativeCompletion> completion(rust::Str module, std::uint32_t line, std::uint32_t column);

        rust::Vec<NativeCompletion>
        completion_detail(rust::Str module, std::uint32_t line, std::uint32_t column, std::size_t index);

        rust::Vec<NativeSignature> signatures(rust::Str module, std::uint32_t line, std::uint32_t column);

        rust::Vec<SourceLocation>
        definitions(rust::Str module, std::uint32_t line, std::uint32_t column, bool resolve_types);

        rust::Vec<Occurrence> occurrences(rust::Str module, rust::Slice<const Occurrence> declarations);

        rust::Vec<Occurrence> rename(
            rust::Str module, std::uint32_t line, std::uint32_t column, rust::Str name,
            rust::Slice<const rust::String> candidates
        );

        rust::Vec<NativeHint> hints_range(rust::Str module, Position start, Position end, bool is_expanded);
        rust::Vec<NativeHint> hint_subjects(rust::Slice<const HintSelector> selectors);
        rust::Vec<Import> imports(rust::Str module);
        std::shared_ptr<Luau::FrontendCancellationToken> cancellation() const;

    private:
        struct Occurrences {
            std::vector<Entry> entries;

            std::map<
                std::tuple<std::string, std::string, Luau::Position, Luau::Position, bool>, std::vector<std::size_t>>
                declarations;
        };

        void initialize();
        void dirty(const std::string &name);
        std::vector<std::string> source_identities(rust::Str module);
        std::vector<std::string> prepare(rust::Str module);

        rust::Vec<NativeCompletion>
        complete(rust::Str module, std::uint32_t line, std::uint32_t column, std::optional<std::size_t> selected);

        const Context &callbacks;
        Flags flags;
        Host host;
        Luau::FrontendOptions options;
        std::unique_ptr<Luau::Frontend> frontend;
        std::unordered_map<std::string, rust::Vec<Diagnostic>> environments;
        std::unordered_map<std::string, Occurrences> indexes;
    };

    std::unique_ptr<Frontend> create(const Context &host, bool retain_types);
}

#endif
