#ifndef INSTAR_LUAU_NATIVE_FRONTEND
#define INSTAR_LUAU_NATIVE_FRONTEND

#include "Luau/Frontend.h"
#include "host.hpp"
#include "rust/cxx.h"

#include <memory>
#include <string>
#include <unordered_map>

namespace instar::luau {
    struct Diagnostic;
    struct Flag;

    class Frontend {
    public:
        Frontend(const Context &context, rust::Slice<const Flag> flags, const Luau::FrontendOptions &options);

        rust::Vec<Diagnostic> check(rust::Str module);
        void invalidate(rust::Str module);
        void clear();
        void environment(const std::string &name);
        const Instances *hierarchy(const std::string &name);

    private:
        void initialize();

        Flags flags;
        Host host;
        Luau::FrontendOptions options;
        std::unique_ptr<Luau::Frontend> frontend;
        std::unordered_map<std::string, rust::Vec<Diagnostic>> environments;
    };

    std::unique_ptr<Frontend> create(const Context &host, bool retainTypes);
}

#endif
