#include "cancellation.hpp"

#include <limits>
#include <mutex>
#include <stdexcept>
#include <unordered_map>

namespace instar::luau {
    namespace {
        std::mutex mutex;
        std::uint64_t next = 0;
        std::unordered_map<std::uint64_t, std::shared_ptr<Luau::FrontendCancellationToken>> tokens;
    }

    std::uint64_t retain(const std::shared_ptr<Luau::FrontendCancellationToken> &token) {
        std::lock_guard<std::mutex> lock(mutex);

        if (next == std::numeric_limits<std::uint64_t>::max()) {
            throw std::runtime_error("native cancellation handle limit exceeded");
        }

        const auto identifier = ++next;
        tokens.emplace(identifier, token);

        return identifier;
    }

    void cancel(std::uint64_t identifier) {
        std::lock_guard<std::mutex> lock(mutex);

        if (const auto found = tokens.find(identifier); found != tokens.end()) {
            found->second->cancel();
        }
    }

    void reset(std::uint64_t identifier) {
        std::lock_guard<std::mutex> lock(mutex);

        if (const auto found = tokens.find(identifier); found != tokens.end()) {
            found->second->cancelled.store(false);
        }
    }

    void release(std::uint64_t identifier) {
        std::lock_guard<std::mutex> lock(mutex);
        tokens.erase(identifier);
    }
}
