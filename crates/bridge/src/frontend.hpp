#ifndef INSTAR_FRONTEND_HEADER
#define INSTAR_FRONTEND_HEADER

#include "rust/cxx.h"
#include <memory>

namespace instar {
    struct Host;
    struct NativeLink;
    class NativeConfiguration;

    class NativeFrontend final {
      public:
        NativeFrontend();
        ~NativeFrontend();
        void configure(rust::Str name, const NativeConfiguration &configuration);
        rust::Vec<NativeLink> prepare(const Host &host, rust::Slice<const rust::String> names);
        void invalidate(rust::Slice<const rust::String> names);

      private:
        struct State;
        std::unique_ptr<State> state;
    };

    std::unique_ptr<NativeFrontend> create_frontend();
} // namespace instar

#endif
