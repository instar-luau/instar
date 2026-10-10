#ifndef INSTAR_CONFIGURATION_HEADER
#define INSTAR_CONFIGURATION_HEADER

namespace instar {
    struct NativeConfigurationInput;
    struct NativeConfigurationResult;
    struct NativeOperation;

    NativeConfigurationResult
    evaluate_configuration(const NativeConfigurationInput &input, const NativeOperation &operation);
}

#endif
