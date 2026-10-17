#ifndef INSTAR_LUAU_NATIVE_FRONTEND
#define INSTAR_LUAU_NATIVE_FRONTEND

#include "Luau/Frontend.h"
#include "host.hpp"
#include "rust/cxx.h"

#include <cstddef>
#include <memory>
#include <string>
#include <unordered_map>

namespace instar::luau {
    struct Diagnostic;
    struct Flag;

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

    private:
        void initialize();

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
