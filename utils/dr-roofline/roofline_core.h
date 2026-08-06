/* C mirror of the roofline-core FFI (utils/roofline-core/src/capi.rs).
 * Keep in sync. */
#ifndef MINIPERF_ROOFLINE_CORE_H
#define MINIPERF_ROOFLINE_CORE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    RC_TARGET_X86 = 0,
    RC_TARGET_RISCV = 1,
    RC_TARGET_AARCH64 = 2,
};

enum {
    RC_KIND_NONE = 0,
    RC_KIND_STATIC = 1,
    RC_KIND_RVV = 2,
    RC_KIND_UNCLASSIFIED = 3,
};

enum {
    RC_FLOW_NORMAL = 0,
    RC_FLOW_CALL = 1,
    RC_FLOW_RETURN = 2,
};

typedef struct rc_cost_t {
    uint64_t scalar_int;
    uint64_t scalar_float;
    uint64_t scalar_double;
    uint64_t vector_int;
    uint64_t vector_float;
    uint64_t vector_double;
} rc_cost_t;

typedef struct rc_classification_t {
    uint32_t kind;
    rc_cost_t cost;
    uint32_t rvv_is_float;
    uint32_t rvv_masked;
    uint64_t rvv_factor;
    uint64_t rvv_sew_scale;
} rc_classification_t;

typedef struct rc_session_t rc_session_t;

rc_session_t *
rc_session_new(const char *output, uint64_t cache_line, uint64_t llc_size,
               uint64_t llc_associativity, uint32_t memory_profile);

void
rc_session_set_image(rc_session_t *session, uint64_t start, uint64_t end,
                     uint64_t entry);

void
rc_classify(uint32_t target, const char *disassembly, rc_classification_t *out);

uint32_t
rc_flow_kind(uint32_t target, const char *disassembly);

void
rc_block_exec(rc_session_t *session, uint32_t thread, uint64_t vaddr,
              uint64_t end_vaddr, uint32_t flow, const rc_cost_t *cost,
              uint64_t instructions);

void
rc_mem_access(rc_session_t *session, uint32_t thread, uint64_t block,
              uint64_t address, uint64_t size, uint32_t is_store);

void
rc_rvv_exec(rc_session_t *session, uint64_t block, uint32_t is_float,
            uint64_t sew_bits, uint64_t operations);

void
rc_unclassified(rc_session_t *session, uint64_t block);

void
rc_rvv_state_error(rc_session_t *session);

int64_t
rc_active_elements(uint64_t vstart, uint64_t vl, const uint8_t *mask,
                   uint64_t mask_len);

int64_t
rc_rvv_sew(uint64_t vtype, uint32_t xlen);

int32_t
rc_finalize(rc_session_t *session);

#ifdef __cplusplus
}
#endif

#endif /* MINIPERF_ROOFLINE_CORE_H */
