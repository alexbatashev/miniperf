/* miniperf user-event tracing API.
 *
 * Link libmperf_trace.a (the static stub) into your application, or load a
 * proxy that forwards here. Every call is a no-op unless the process runs
 * under `mperf record` (MPERF_SESSION_DIR set and the collector core
 * loadable). See docs/event-collection-redesign.md.
 */
#ifndef MPERF_TRACE_H
#define MPERF_TRACE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { MPERF_TRACE_FLAG_STACK = 1u };

typedef struct mperf_trace_payload {
    const char *name;
    const char *function;
    const char *file;
    uint32_t line;
    uint32_t column;
    uint32_t flags;
} mperf_trace_payload_t;

typedef struct mperf_trace_handle mperf_trace_handle_t;

mperf_trace_handle_t *mperf_trace_register(const mperf_trace_payload_t *payload);
uint64_t mperf_trace_begin(mperf_trace_handle_t *handle, uint64_t parent);
void mperf_trace_end(mperf_trace_handle_t *handle, uint64_t instance);
void mperf_trace_instant(mperf_trace_handle_t *handle, int64_t value);
void mperf_trace_counter(mperf_trace_handle_t *handle, int64_t value);
void mperf_trace_shutdown(void);

#define MPERF_TRACE_POINT(var, event_name, event_flags)                        \
    static mperf_trace_handle_t *var;                                          \
    if (!var) {                                                                \
        mperf_trace_payload_t payload = {event_name, __func__, __FILE__,       \
                                         __LINE__, 0,          event_flags};   \
        var = mperf_trace_register(&payload);                                  \
    }

#define MPERF_INSTANT(event_name, value)                                       \
    do {                                                                       \
        MPERF_TRACE_POINT(mperf_handle_, event_name, 0)                        \
        mperf_trace_instant(mperf_handle_, (value));                           \
    } while (0)

#define MPERF_COUNTER(event_name, value)                                       \
    do {                                                                       \
        MPERF_TRACE_POINT(mperf_handle_, event_name, 0)                        \
        mperf_trace_counter(mperf_handle_, (value));                           \
    } while (0)

#ifdef __cplusplus
}

namespace mperf {
class Scope {
  public:
    Scope(mperf_trace_handle_t *handle, uint64_t parent = 0)
        : handle_(handle), instance_(mperf_trace_begin(handle, parent)) {}
    ~Scope() { mperf_trace_end(handle_, instance_); }
    Scope(const Scope &) = delete;
    Scope &operator=(const Scope &) = delete;
    uint64_t instance() const { return instance_; }

  private:
    mperf_trace_handle_t *handle_;
    uint64_t instance_;
};
} // namespace mperf

#define MPERF_SCOPE(event_name)                                                \
    static mperf_trace_handle_t *mperf_scope_handle_ = nullptr;                \
    if (!mperf_scope_handle_) {                                                \
        mperf_trace_payload_t mperf_scope_payload_ = {                         \
            event_name, __func__, __FILE__, __LINE__, 0, 0};                   \
        mperf_scope_handle_ = mperf_trace_register(&mperf_scope_payload_);     \
    }                                                                          \
    mperf::Scope mperf_scope_(mperf_scope_handle_)
#endif

#endif /* MPERF_TRACE_H */
