#ifndef MINIPERF_MANUAL_ROOFLINE_H
#define MINIPERF_MANUAL_ROOFLINE_H

#include <stdint.h>

struct mperf_loop_info {
    uint32_t line;
    const char *filename;
    const char *func_name;
};

struct mperf_loop_stats {
    uint64_t trip_count;
    uint64_t bytes_load;
    uint64_t bytes_store;
    uint64_t scalar_int_ops;
    uint64_t scalar_float_ops;
    uint64_t scalar_double_ops;
    uint64_t vector_int_ops;
    uint64_t vector_float_ops;
    uint64_t vector_double_ops;
};

int mperf_roofline_internal_is_instrumented_profiling(void);
void *mperf_roofline_internal_notify_loop_begin(const struct mperf_loop_info *info);
void mperf_roofline_internal_notify_loop_stats(
    void *handle, const struct mperf_loop_stats *stats);
void mperf_roofline_internal_notify_loop_end(void *handle);

static inline void *bench_roofline_begin(uint32_t line, const char *filename) {
    const struct mperf_loop_info info = {line, filename, "main"};
    return mperf_roofline_internal_notify_loop_begin(&info);
}

static inline void bench_roofline_end(void *handle,
                                      const struct mperf_loop_stats *stats) {
    if (handle == 0) {
        return;
    }
    if (mperf_roofline_internal_is_instrumented_profiling()) {
        mperf_roofline_internal_notify_loop_stats(handle, stats);
    }
    mperf_roofline_internal_notify_loop_end(handle);
}

#endif
