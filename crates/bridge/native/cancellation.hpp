#ifndef INSTAR_LUAU_NATIVE_CANCELLATION
#define INSTAR_LUAU_NATIVE_CANCELLATION

#include "Luau/Cancellation.h"

#include <cstdint>
#include <memory>

namespace instar::luau {
    std::uint64_t retain(const std::shared_ptr<Luau::FrontendCancellationToken> &token);
    void cancel(std::uint64_t identifier);
    void reset(std::uint64_t identifier);
    void release(std::uint64_t identifier);
}

#endif
