#ifndef OPERATION_HPP
#define OPERATION_HPP

#include "instar-bridge/src/bridge.rs.h"

#include <exception>
#include <memory>
#include <stdexcept>
#include <string>

namespace instar {
    struct CallbackFailure final : std::exception {
        const char *what() const noexcept override { return "native callback failed"; }
    };

    struct DefinitionFailure final : std::exception {
        const char *what() const noexcept override { return "definition loading failed"; }
    };

    inline Failure failure(FailureKind kind, const char *message) noexcept {
        Failure result{kind, nullptr};

        try {
            if (message) {
                result.message = std::make_unique<std::string>(message);
            }
        } catch (...) {
        }

        return result;
    }

    template <typename Function> Failure attempt(Function &&function) noexcept {
        try {
            function();

            return {FailureKind::Success, nullptr};
        } catch (const CallbackFailure &error) {
            return failure(FailureKind::Callback, error.what());
        } catch (const DefinitionFailure &error) {
            return failure(FailureKind::Definition, error.what());
        } catch (const std::exception &error) {
            return failure(FailureKind::Operation, error.what());
        } catch (...) {
            return failure(FailureKind::Operation, nullptr);
        }
    }

    class Operation {
      public:
        Operation(Checker &checker, Host &host) : checker(checker) {
            if (checker.host) {
                throw std::logic_error("reentrant checker operation");
            }

            checker.host = &host;
        }

        ~Operation() { checker.host = nullptr; }
        Operation(const Operation &) = delete;
        Operation &operator=(const Operation &) = delete;
        Luau::Frontend &frontend() { return checker.frontend; }

      private:
        Checker &checker;
    };
} // namespace instar

#endif
