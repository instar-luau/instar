#include "cancellation.hpp"

namespace instar::luau {
    void cancel(const std::shared_ptr<Luau::FrontendCancellationToken> &token) {
        token->cancel();
    }

    void reset(const std::shared_ptr<Luau::FrontendCancellationToken> &token) {
        token->cancelled.store(false);
    }
}
