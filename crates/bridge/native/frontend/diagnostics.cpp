#include "diagnostics.hpp"

#include <stdexcept>

namespace instar::frontend {
    size_t offset(const Host &host, const std::string &name, Luau::Position position) {
        return host.offset(name, position.line, position.column);
    }

    NativeKind kind(const Luau::TypeError &error) {
        if (Luau::get<Luau::SyntaxError>(error)) {
            return NativeKind::Syntax;
        }

        if (Luau::get<Luau::UnknownRequire>(error) || Luau::get<Luau::IllegalRequire>(error)) {
            return NativeKind::Resolution;
        }

        if (Luau::get<Luau::CodeTooComplex>(error) || Luau::get<Luau::UnificationTooComplex>(error) ||
            Luau::get<Luau::NormalizationTooComplex>(error) ||
            Luau::get<Luau::ConstraintSolvingIncompleteError>(error) || Luau::get<Luau::InternalError>(error)) {
            return NativeKind::Analysis;
        }

        return NativeKind::Type;
    }

    NativeLocation location(const Host &host, const Luau::ModuleName &name, Luau::Location range) {
        const NativeDocument source = host.read_document(name);

        if (!source.found) {
            throw std::runtime_error("native diagnostic source is unavailable: " + name);
        }

        return NativeLocation{rust::String(name),
            source.revision,
            offset(host, name, range.begin),
            offset(host, name, range.end)};
    }

    NativeDiagnostic diagnostic(const Host &host, const Luau::TypeError &error, const std::string &fallback) {
        const std::string name = error.moduleName.empty() ? fallback : error.moduleName;

        NativeDiagnostic result{location(host, name, error.location),
            kind(error),
            error.code(),
            rust::String(Luau::toString(error)),
            {}};

        const auto *count = Luau::get<Luau::CountMismatch>(error);

        if (Luau::get<Luau::GenericError>(error) ||
            (count && count->context == Luau::CountMismatch::Arg && count->expected == 1 && count->actual != 1)) {
            if (host.require(name, result.location.start, result.location.end)) {
                result.kind = NativeKind::Resolution;
            }
        }

        if (const auto *duplicate = Luau::get<Luau::DuplicateTypeDefinition>(error);
            duplicate && duplicate->previousLocation) {
            result.related.push_back(
                NativeRelated{location(host, name, *duplicate->previousLocation),
                    rust::String("Previous type definition")}
            );
        }

        const Luau::TypeError *nested = &error;

        while (const auto *mismatch = Luau::get<Luau::TypeMismatch>(*nested)) {
            if (!mismatch->error) {
                break;
            }

            nested = mismatch->error.get();
            const std::string related_name = nested->moduleName.empty() ? name : nested->moduleName;

            result.related.push_back(
                NativeRelated{location(host, related_name, nested->location), rust::String(Luau::toString(*nested))}
            );
        }

        return result;
    }
}
