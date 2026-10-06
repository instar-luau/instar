#include "diagnostics.hpp"
#include <stdexcept>

namespace instar::frontend {
    size_t offset(const std::string &source, Luau::Position position) {
        size_t start = 0;

        for (unsigned int line = 0; line < position.line; ++line) {
            const size_t newline = source.find('\n', start);

            if (newline == std::string::npos) {
                throw std::runtime_error("native source position exceeds host revision");
            }

            start = newline + 1;
        }

        if (position.column > source.size() - start) {
            throw std::runtime_error("native source column exceeds host revision");
        }

        return start + position.column;
    }

    NativeLocation location(const Host &host, const Luau::ModuleName &name, Luau::Location range) {
        const NativeSource source = host.read_source(name);

        if (!source.found) {
            throw std::runtime_error("native diagnostic source is unavailable: " + name);
        }

        const std::string bytes(source.text);

        return NativeLocation{rust::String(name),
            source.revision,
            offset(bytes, range.begin),
            offset(bytes, range.end)};
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
            const NativeSource source = host.read_source(name);

            for (const NativeSite &site : source.sites) {
                if (site.call_start == result.location.start && site.call_end == result.location.end) {
                    result.kind = NativeKind::Resolution;
                    break;
                }
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
            const std::string relatedName = nested->moduleName.empty() ? name : nested->moduleName;

            result.related.push_back(
                NativeRelated{location(host, relatedName, nested->location), rust::String(Luau::toString(*nested))}
            );
        }

        return result;
    }

} // namespace instar::frontend
