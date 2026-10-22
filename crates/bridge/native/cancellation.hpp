#ifndef INSTAR_LUAU_NATIVE_CANCELLATION
#define INSTAR_LUAU_NATIVE_CANCELLATION

#include "Luau/Cancellation.h"

#include <memory>

namespace instar::luau {
    void cancel(const std::shared_ptr<Luau::FrontendCancellationToken> &token);
    void reset(const std::shared_ptr<Luau::FrontendCancellationToken> &token);
}

#endif
