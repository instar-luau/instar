#ifndef INSTAR_LUAU_NATIVE_FRONTEND
#define INSTAR_LUAU_NATIVE_FRONTEND

#include "Luau/Frontend.h"
#include "host.hpp"
#include "rust/cxx.h"

#include <memory>
#include <string>
#include <unordered_map>

namespace instar::luau {
    struct Analysis;
    struct Diagnostic;
    struct Flag;
    struct Warning;

    class Frontend {
    public:
        Frontend(const Context &context, rust::Slice<const Flag> flags, const Luau::FrontendOptions &options);

        Analysis analyze(rust::Str module, bool lint);
        void invalidate(rust::Str module);
        void clear();
        void environment(const std::string &name);
        const Instances *hierarchy(const std::string &name);

    private:
        void initialize();

        Flags flags;
        Host host;
        Luau::FrontendOptions options;
        bool linting = false;
        std::unique_ptr<Luau::Frontend> frontend;
        std::unordered_map<std::string, rust::Vec<Diagnostic>> environments;
        std::unordered_map<std::string, rust::Vec<Warning>> findings;
    };

    std::unique_ptr<Frontend> create(const Context &host, bool retainTypes);
    rust::Vec<rust::String> warnings();
}

#endif
