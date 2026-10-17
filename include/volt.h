#ifndef VOLT_H
#define VOLT_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct VoltEngine VoltEngine;
typedef int (*VoltNumberCallback)(
    void *user_data,
    const double *arguments,
    size_t argument_count,
    double *out_result
);

enum VoltStatus {
    VOLT_STATUS_OK = 0,
    VOLT_STATUS_INVALID_ARGUMENT = 1,
    VOLT_STATUS_EVALUATION_ERROR = 2,
    VOLT_STATUS_INVALID_OUTPUT = 3,
    VOLT_STATUS_INTERNAL_ERROR = 4
};

VoltEngine *volt_engine_new(void);
VoltEngine *volt_engine_new_with_limits(
    size_t max_source_bytes,
    size_t max_execution_steps,
    size_t max_call_depth
);
VoltEngine *volt_engine_new_with_budgets(
    size_t max_source_bytes,
    size_t max_execution_steps,
    size_t max_call_depth,
    unsigned long long max_evaluation_time_ms
);
void volt_engine_free(VoltEngine *engine);
int volt_engine_register_number_fn(
    VoltEngine *engine,
    const char *name,
    VoltNumberCallback callback,
    void *user_data
);

int volt_engine_eval(
    VoltEngine *engine,
    const char *source,
    char **out_result
);
const char *volt_engine_last_error(const VoltEngine *engine);
void volt_string_free(char *value);

#ifdef __cplusplus
}
#endif

#endif
