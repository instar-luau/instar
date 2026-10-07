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

    namespace frontend {
        struct State;
    }

    class NativeFrontend final {
      public:
        NativeFrontend();
        ~NativeFrontend();
        void configure(rust::Str name, const NativeConfiguration &configuration);
        rust::Vec<NativeLink> prepare(const Host &host, rust::Slice<const rust::String> names);
        void invalidate(rust::Slice<const rust::String> names);
        rust::String documentation(rust::Str module, rust::Str symbol) const;

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
        std::unique_ptr<frontend::State> state;
    };

    std::unique_ptr<NativeFrontend> create_frontend();
} // namespace instar

#endif
