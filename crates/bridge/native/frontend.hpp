#ifndef INSTAR_LUAU_NATIVE_FRONTEND
#define INSTAR_LUAU_NATIVE_FRONTEND

#include "Luau/Frontend.h"
#include "host.hpp"
#include "rust/cxx.h"

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <unordered_map>

namespace instar::luau {
    struct Diagnostic;
    struct Flag;
    struct Hover;
    struct Completion;
    struct Signature;
    struct SourceLocation;
    struct Occurrence;
    struct Hint;
    struct Import;

    class Frontend {
    public:
        Frontend(const Context &context, rust::Slice<const Flag> settings, const Luau::FrontendOptions &options);

        rust::Vec<Diagnostic> analyze(rust::Str module);
        void queue(rust::Str module);
        std::size_t checks() const;
        void invalidate(rust::Str module);
        void refresh();
        void clear();
        void environment(const std::string &name);
        const Instances *hierarchy(const std::string &name);
        rust::Vec<Hover> hover(rust::Str module, std::uint32_t line, std::uint32_t column);
        rust::Vec<Completion> completion(rust::Str module, std::uint32_t line, std::uint32_t column);
        rust::Vec<Signature> signatures(rust::Str module, std::uint32_t line, std::uint32_t column);
        rust::Vec<SourceLocation> definitions(rust::Str module, std::uint32_t line, std::uint32_t column, bool types);
        rust::Vec<Occurrence> occurrences(rust::Str module);

        rust::Vec<Occurrence> rename(
            rust::Str module, std::uint32_t line, std::uint32_t column, rust::Str name,
            rust::Slice<const rust::String> candidates
        );

        rust::Vec<Hint> hints(rust::Str module);
        rust::Vec<Import> imports(rust::Str module);
        std::uint64_t cancellation() const;

    private:
        void initialize();
        std::vector<std::string> sourceIdentities(rust::Str module);
        std::vector<std::string> prepare(rust::Str module);

        const Context &callbacks;
        Flags flags;
        Host host;
        Luau::FrontendOptions options;
        std::unique_ptr<Luau::Frontend> frontend;
        std::unordered_map<std::string, rust::Vec<Diagnostic>> environments;
    };

    std::unique_ptr<Frontend> create(const Context &host, bool retain_types);
}

#endif
