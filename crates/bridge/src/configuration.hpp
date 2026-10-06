#ifndef INSTAR_CONFIGURATION_HEADER
#define INSTAR_CONFIGURATION_HEADER

#include "rust/cxx.h"

#include <memory>

namespace Luau {
    struct Config;
}

namespace instar {
    struct NativeOutcome;
    struct NativeSnapshot;
    struct NativeFlag;
    void validate_flags(rust::Slice<const NativeFlag> flags);
    void apply_flags(rust::Slice<const NativeFlag> flags);
    rust::Vec<NativeFlag> normalize_flags(rust::Slice<const NativeFlag> flags);

    class NativeConfiguration final {
      public:
        NativeConfiguration();
        ~NativeConfiguration();
        NativeOutcome apply(rust::Str source, rust::Str path, bool executable, double timeoutSeconds);
        NativeSnapshot snapshot() const;
        void restore(const NativeSnapshot &snapshot);
        const Luau::Config &value() const;

      private:
        std::unique_ptr<Luau::Config> configuration;
    };

    std::unique_ptr<NativeConfiguration> create();
} // namespace instar

#endif
