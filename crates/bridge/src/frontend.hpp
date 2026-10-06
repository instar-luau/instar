#ifndef INSTAR_FRONTEND_HEADER
#define INSTAR_FRONTEND_HEADER

#include "rust/cxx.h"
#include <memory>

namespace instar {
    struct Host;
    struct Cancellation;
    struct NativeCheck;
    struct NativeLintResult;
    struct NativeLink;
    class NativeConfiguration;

    class NativeFrontend final {
      public:
        NativeFrontend();
        ~NativeFrontend();
        void configure(rust::Str name, const NativeConfiguration &configuration);
        rust::Vec<NativeLink> prepare(const Host &host, rust::Slice<const rust::String> names);
        void invalidate(rust::Slice<const rust::String> names);

        NativeCheck check(
            const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
            rust::Slice<const rust::String> names, const Cancellation &cancellation
        );

        NativeLintResult lint(
            const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
            rust::Slice<const rust::String> names, const Cancellation &cancellation,
            rust::Slice<const rust::String> semantic_modules
        );

      private:
        NativeCheck analyze(
            const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
            rust::Slice<const rust::String> names, const Cancellation &cancellation, bool retain_types, bool typecheck
        );

        struct State;
        std::unique_ptr<State> state;
    };

    std::unique_ptr<NativeFrontend> create_frontend();
} // namespace instar

#endif
