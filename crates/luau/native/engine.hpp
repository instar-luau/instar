#ifndef INSTAR_ENGINE_HEADER
#define INSTAR_ENGINE_HEADER

#include <memory>

namespace instar {
    struct NativeAnalysisRequest;
    struct NativeAnalysisResult;
    struct NativeFactsInput;
    struct NativeFactsResult;
    struct NativeLintInput;
    struct NativeLintResult;
    struct NativePositionInput;
    struct NativeInspectionResult;
    struct NativeCompletions;
    struct NativeOperation;

    class Engine final {
      public:
        Engine();
        ~Engine();
        NativeAnalysisResult analyze(const NativeAnalysisRequest &input, const NativeOperation &operation);
        NativeFactsResult facts(const NativeFactsInput &input, const NativeOperation &operation);
        NativeLintResult lint(const NativeLintInput &input, const NativeOperation &operation);
        NativeInspectionResult inspect(const NativePositionInput &input, const NativeOperation &operation);
        NativeCompletions complete(const NativePositionInput &input, const NativeOperation &operation);

      private:
        struct State;
        std::unique_ptr<State> state;
    };

    std::unique_ptr<Engine> create_engine();
}

#endif
