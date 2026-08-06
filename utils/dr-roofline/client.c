/* DynamoRIO client for miniperf roofline/memory accounting.
 *
 * All analysis lives in the shared roofline-core Rust staticlib (see
 * roofline_core.h); this file is the DynamoRIO instrumentation adapter. It
 * emits the same three artifacts as the QEMU plugin:
 *   <output>            key=value counters
 *   <output minus .counts>.cfg          dynamic CFG v3
 *   <output minus .counts>.memory.json  memory profile (when enabled)
 *
 * Client options (drrun -c libdr_roofline.so <options> -- app):
 *   output=<path> cache-line=<n> llc-size=<n> llc-assoc=<n>
 *   memory-profile=on|off
 */

#include "dr_api.h"
#include "drmgr.h"
#include "drutil.h"
#include "drreg.h"
#include "roofline_core.h"
#include <string.h>
#include <stdlib.h>

#define DEFAULT_CACHE_LINE 64
#define DEFAULT_LLC_SIZE (8ULL * 1024 * 1024)
#define DEFAULT_LLC_ASSOC 16

static rc_session_t *session;
static uint32_t target;
static bool in_child; /* forked child: do not write artifacts */
static bool debug_unclassified;
static bool debug_classify;

static int tls_index = -1;
static uint thread_counter;

typedef struct bb_data_t {
    uint64_t vaddr;
    uint64_t end_vaddr;
    uint32_t flow;
    rc_cost_t cost;
    uint64_t instructions;
    struct bb_data_t *next;
} bb_data_t;

/* Per-instruction data referenced from instrumented code. Clean-call
 * argument lists are kept to one or two operands: materializing several
 * 64-bit immediates per instruction can exceed DynamoRIO's basic-block
 * instrumentation size limit (seen on riscv64). */
typedef struct mem_info_t {
    uint64_t block;
    uint32_t size;
    uint32_t is_store;
    struct mem_info_t *next;
} mem_info_t;

typedef struct rvv_info_t {
    uint64_t block;
    uint32_t is_float;
    uint32_t masked;
    uint64_t factor;
    uint64_t sew_scale;
    struct rvv_info_t *next;
} rvv_info_t;

static void *bb_list_lock;
static bb_data_t *bb_list;
static mem_info_t *mem_list;
static rvv_info_t *rvv_list;

static uint32_t
thread_index(void *drcontext)
{
    return (uint32_t)(uintptr_t)drmgr_get_tls_field(drcontext, tls_index);
}

/* ---------------- events fired from instrumented code ---------------- */

static void
block_exec_event(bb_data_t *data)
{
    if (in_child)
        return;
    void *drcontext = dr_get_current_drcontext();
    rc_block_exec(session, thread_index(drcontext), data->vaddr, data->end_vaddr,
                  data->flow, &data->cost, data->instructions);
}

static void
mem_event(mem_info_t *info, app_pc address)
{
    if (in_child)
        return;
    void *drcontext = dr_get_current_drcontext();
    rc_mem_access(session, thread_index(drcontext), info->block,
                  (uint64_t)(uintptr_t)address, info->size, info->is_store);
}

static void
unclassified_event(uint64_t block)
{
    if (in_child)
        return;
    rc_unclassified(session, block);
}

#ifdef RISCV64
static void
rvv_event(rvv_info_t *info)
{
    if (in_child)
        return;
    void *drcontext = dr_get_current_drcontext();
    dr_mcontext_t mc = { sizeof(mc), DR_MC_ALL };
    if (!dr_get_mcontext(drcontext, &mc)) {
        rc_rvv_state_error(session);
        return;
    }
    int64_t sew = rc_rvv_sew(mc.vtype, 64);
    if (sew < 0 || info->sew_scale == 0 ||
        (uint64_t)sew > UINT64_MAX / info->sew_scale) {
        rc_rvv_state_error(session);
        return;
    }
    int64_t elements;
    if (info->masked) {
        elements = rc_active_elements(mc.vstart, mc.vl,
                                      (const uint8_t *)mc.simd[0].u64,
                                      sizeof(mc.simd[0]));
    } else {
        elements = rc_active_elements(mc.vstart, mc.vl, NULL, 0);
    }
    if (elements < 0) {
        rc_rvv_state_error(session);
        return;
    }
    rc_rvv_exec(session, info->block, info->is_float,
                (uint64_t)sew * info->sew_scale,
                (uint64_t)elements * info->factor);
}
#endif

/* ---------------- translation-time helpers ---------------- */

static void
disassemble_instr(void *drcontext, instr_t *instr, char *buffer, size_t size)
{
    buffer[0] = '\0';
    instr_disassemble_to_buffer(drcontext, instr, buffer, size);
    buffer[size - 1] = '\0';
}

static uint32_t
flow_kind(instr_t *instr)
{
    if (instr_is_return(instr))
        return RC_FLOW_RETURN;
    if (instr_is_call(instr))
        return RC_FLOW_CALL;
    return RC_FLOW_NORMAL;
}

/* ---------------- bb instrumentation ---------------- */

static dr_emit_flags_t
event_bb_app2app(void *drcontext, void *tag, instrlist_t *bb, bool for_trace,
                 bool translating)
{
    if (!drutil_expand_rep_string(drcontext, bb))
        DR_ASSERT(false);
    return DR_EMIT_DEFAULT;
}

static dr_emit_flags_t
event_bb_analysis(void *drcontext, void *tag, instrlist_t *bb, bool for_trace,
                  bool translating, void **user_data)
{
    bb_data_t *data = dr_global_alloc(sizeof(bb_data_t));
    char text[256];
    instr_t *instr;
    rc_classification_t cls;

    memset(data, 0, sizeof(*data));
    for (instr = instrlist_first_app(bb); instr != NULL;
         instr = instr_get_next_app(instr)) {
        data->instructions++;
        if (data->vaddr == 0)
            data->vaddr = (uint64_t)(uintptr_t)instr_get_app_pc(instr);
        if (instr_get_next_app(instr) == NULL) {
            data->end_vaddr = (uint64_t)(uintptr_t)instr_get_app_pc(instr) +
                instr_length(drcontext, instr);
            data->flow = flow_kind(instr);
        }
        disassemble_instr(drcontext, instr, text, sizeof(text));
        rc_classify(target, text, &cls);
        if (cls.kind == RC_KIND_STATIC) {
            data->cost.scalar_int += cls.cost.scalar_int;
            data->cost.scalar_float += cls.cost.scalar_float;
            data->cost.scalar_double += cls.cost.scalar_double;
            data->cost.vector_int += cls.cost.vector_int;
            data->cost.vector_float += cls.cost.vector_float;
            data->cost.vector_double += cls.cost.vector_double;
        }
    }

    dr_mutex_lock(bb_list_lock);
    data->next = bb_list;
    bb_list = data;
    dr_mutex_unlock(bb_list_lock);
    *user_data = data;
    return DR_EMIT_DEFAULT;
}

static void
instrument_mem_refs(void *drcontext, instrlist_t *bb, instr_t *instr,
                    uint64_t block)
{
    int i;
    reg_id_t reg_addr, reg_scratch;
    bool reserved = false;

    for (i = 0; i < instr_num_srcs(instr) + instr_num_dsts(instr); i++) {
        bool is_store = i >= instr_num_srcs(instr);
        opnd_t op = is_store ? instr_get_dst(instr, i - instr_num_srcs(instr))
                             : instr_get_src(instr, i);
        if (!opnd_is_memory_reference(op))
            continue;
        /* Skip address-generation pseudo-references (x86 lea). */
        if (!is_store && !instr_reads_memory(instr))
            continue;
        if (is_store && !instr_writes_memory(instr))
            continue;
        uint size = opnd_size_in_bytes(opnd_get_size(op));
        if (size == 0)
            continue;
        if (!reserved) {
            if (drreg_reserve_register(drcontext, bb, instr, NULL, &reg_addr) !=
                    DRREG_SUCCESS ||
                drreg_reserve_register(drcontext, bb, instr, NULL, &reg_scratch) !=
                    DRREG_SUCCESS) {
                DR_ASSERT(false);
                return;
            }
            reserved = true;
        }
        if (!drutil_insert_get_mem_addr(drcontext, bb, instr, op, reg_addr,
                                        reg_scratch))
            continue;
        mem_info_t *info = dr_global_alloc(sizeof(mem_info_t));
        info->block = block;
        info->size = size;
        info->is_store = is_store ? 1 : 0;
        dr_mutex_lock(bb_list_lock);
        info->next = mem_list;
        mem_list = info;
        dr_mutex_unlock(bb_list_lock);
        dr_insert_clean_call(drcontext, bb, instr, (void *)mem_event, false, 2,
                             OPND_CREATE_INTPTR(info), opnd_create_reg(reg_addr));
    }
    if (reserved) {
        drreg_unreserve_register(drcontext, bb, instr, reg_scratch);
        drreg_unreserve_register(drcontext, bb, instr, reg_addr);
    }
}

static dr_emit_flags_t
event_bb_insertion(void *drcontext, void *tag, instrlist_t *bb, instr_t *instr,
                   bool for_trace, bool translating, void *user_data)
{
    bb_data_t *data = (bb_data_t *)user_data;
    char text[256];
    rc_classification_t cls;

    if (!instr_is_app(instr))
        return DR_EMIT_DEFAULT;

    if (instr == instrlist_first_app(bb)) {
        dr_insert_clean_call(drcontext, bb, instr, (void *)block_exec_event,
                             false, 1, OPND_CREATE_INTPTR(data));
    }

    disassemble_instr(drcontext, instr, text, sizeof(text));
    rc_classify(target, text, &cls);
    if (debug_classify) {
        dr_fprintf(STDERR,
                   "miniperf dr-roofline: classify kind=%u si=%llu sf=%llu sd=%llu vi=%llu vf=%llu vd=%llu '%s'\n",
                   cls.kind, cls.cost.scalar_int, cls.cost.scalar_float,
                   cls.cost.scalar_double, cls.cost.vector_int,
                   cls.cost.vector_float, cls.cost.vector_double, text);
    }
    if (cls.kind == RC_KIND_UNCLASSIFIED) {
        if (debug_unclassified)
            dr_fprintf(STDERR, "miniperf dr-roofline: unclassified '%s'\n", text);
        dr_insert_clean_call(drcontext, bb, instr, (void *)unclassified_event,
                             false, 1, OPND_CREATE_INT64(data->vaddr));
    }
#ifdef RISCV64
    if (cls.kind == RC_KIND_RVV) {
        rvv_info_t *info = dr_global_alloc(sizeof(rvv_info_t));
        info->block = data->vaddr;
        info->is_float = cls.rvv_is_float;
        info->masked = cls.rvv_masked;
        info->factor = cls.rvv_factor;
        info->sew_scale = cls.rvv_sew_scale;
        dr_mutex_lock(bb_list_lock);
        info->next = rvv_list;
        rvv_list = info;
        dr_mutex_unlock(bb_list_lock);
        dr_insert_clean_call(drcontext, bb, instr, (void *)rvv_event, false, 1,
                             OPND_CREATE_INTPTR(info));
    }
#endif

    if (instr_reads_memory(instr) || instr_writes_memory(instr))
        instrument_mem_refs(drcontext, bb, instr, data->vaddr);

    return DR_EMIT_DEFAULT;
}

/* ---------------- lifecycle ---------------- */

static void
event_thread_init(void *drcontext)
{
    uint index = dr_atomic_add32_return_sum((volatile int *)&thread_counter, 1) - 1;
    drmgr_set_tls_field(drcontext, tls_index, (void *)(uintptr_t)index);
}

static void
event_fork_init(void *drcontext)
{
    /* Artifacts describe the root process only, matching the QEMU plugin's
     * child_process_seen semantics. */
    in_child = true;
}

static void
event_exit(void)
{
    if (!in_child && session != NULL)
        rc_finalize(session);
    session = NULL;

    dr_mutex_lock(bb_list_lock);
    while (bb_list != NULL) {
        bb_data_t *next = bb_list->next;
        dr_global_free(bb_list, sizeof(bb_data_t));
        bb_list = next;
    }
    while (mem_list != NULL) {
        mem_info_t *next = mem_list->next;
        dr_global_free(mem_list, sizeof(mem_info_t));
        mem_list = next;
    }
    while (rvv_list != NULL) {
        rvv_info_t *next = rvv_list->next;
        dr_global_free(rvv_list, sizeof(rvv_info_t));
        rvv_list = next;
    }
    dr_mutex_unlock(bb_list_lock);
    dr_mutex_destroy(bb_list_lock);

    drmgr_unregister_tls_field(tls_index);
    drreg_exit();
    drutil_exit();
    drmgr_exit();
}

static uint64_t
parse_u64_option(int argc, const char *argv[], const char *name,
                 uint64_t default_value)
{
    size_t len = strlen(name);
    int i;
    for (i = 1; i < argc; i++) {
        if (strncmp(argv[i], name, len) == 0 && argv[i][len] == '=')
            return strtoull(argv[i] + len + 1, NULL, 10);
    }
    return default_value;
}

static const char *
parse_str_option(int argc, const char *argv[], const char *name,
                 const char *default_value)
{
    size_t len = strlen(name);
    int i;
    for (i = 1; i < argc; i++) {
        if (strncmp(argv[i], name, len) == 0 && argv[i][len] == '=')
            return argv[i] + len + 1;
    }
    return default_value;
}

DR_EXPORT void
dr_client_main(client_id_t id, int argc, const char *argv[])
{
    dr_set_client_name("miniperf dr-roofline", "https://github.com/alexbatashev");

#if defined(X86)
    target = RC_TARGET_X86;
#elif defined(RISCV64)
    target = RC_TARGET_RISCV;
#elif defined(AARCH64)
    target = RC_TARGET_AARCH64;
#else
#    error unsupported architecture
#endif

    const char *output = parse_str_option(argc, argv, "output", "dr-roofline.counts");
    uint64_t cache_line = parse_u64_option(argc, argv, "cache-line", DEFAULT_CACHE_LINE);
    uint64_t llc_size = parse_u64_option(argc, argv, "llc-size", DEFAULT_LLC_SIZE);
    uint64_t llc_assoc = parse_u64_option(argc, argv, "llc-assoc", DEFAULT_LLC_ASSOC);
    const char *profile = parse_str_option(argc, argv, "memory-profile", "on");
    uint32_t memory_profile = strcmp(profile, "off") != 0;
    debug_unclassified = getenv("MPERF_DR_DEBUG_UNCLASSIFIED") != NULL;
    debug_classify = getenv("MPERF_DR_DEBUG_CLASSIFY") != NULL;

    session = rc_session_new(output, cache_line, llc_size, llc_assoc, memory_profile);
    if (session == NULL) {
        dr_fprintf(STDERR,
                   "miniperf dr-roofline: invalid cache-line/llc-size/llc-assoc\n");
        dr_abort();
    }

    module_data_t *main_module = dr_get_main_module();
    if (main_module != NULL) {
        rc_session_set_image(session, (uint64_t)(uintptr_t)main_module->start,
                             (uint64_t)(uintptr_t)main_module->end,
                             (uint64_t)(uintptr_t)main_module->entry_point);
        dr_free_module_data(main_module);
    }

    drreg_options_t ops = { sizeof(ops), 4 /*spill slots*/, false };
    if (!drmgr_init() || !drutil_init() || drreg_init(&ops) != DRREG_SUCCESS)
        DR_ASSERT(false);
    bb_list_lock = dr_mutex_create();
    tls_index = drmgr_register_tls_field();
    DR_ASSERT(tls_index != -1);
    if (!drmgr_register_thread_init_event(event_thread_init) ||
        !drmgr_register_bb_app2app_event(event_bb_app2app, NULL) ||
        !drmgr_register_bb_instrumentation_event(event_bb_analysis,
                                                 event_bb_insertion, NULL))
        DR_ASSERT(false);
    dr_register_fork_init_event(event_fork_init);
    drmgr_register_exit_event(event_exit);
}
