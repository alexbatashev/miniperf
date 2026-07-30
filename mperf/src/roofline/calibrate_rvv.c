#include <stddef.h>
#include <riscv_vector.h>

size_t mperf_roofline_rvv_vlmax(void) {
    return __riscv_vsetvlmax_e64m1();
}

double mperf_roofline_compute_rvv(size_t iterations, size_t worker) {
    const size_t vl = __riscv_vsetvlmax_e64m1();
    const double seed = 0.1 * (double)(worker + 1);
    const vfloat64m1_t multiplier =
        __riscv_vfmv_v_f_f64m1(1.0000000000000002, vl);
    const vfloat64m1_t addend =
        __riscv_vfmv_v_f_f64m1(0.0000000000000001, vl);
    const vfloat64m1_t increment = __riscv_vfmv_v_f_f64m1(0.1, vl);
    vfloat64m1_t a0 = __riscv_vfmv_v_f_f64m1(seed, vl);
    vfloat64m1_t a1 = __riscv_vfadd_vv_f64m1(a0, increment, vl);
    vfloat64m1_t a2 = __riscv_vfadd_vv_f64m1(a1, increment, vl);
    vfloat64m1_t a3 = __riscv_vfadd_vv_f64m1(a2, increment, vl);
    vfloat64m1_t a4 = __riscv_vfadd_vv_f64m1(a3, increment, vl);
    vfloat64m1_t a5 = __riscv_vfadd_vv_f64m1(a4, increment, vl);
    vfloat64m1_t a6 = __riscv_vfadd_vv_f64m1(a5, increment, vl);
    vfloat64m1_t a7 = __riscv_vfadd_vv_f64m1(a6, increment, vl);

    for (size_t iteration = 0; iteration < iterations; ++iteration) {
        a0 = __riscv_vfmacc_vv_f64m1(a0, multiplier, addend, vl);
        a1 = __riscv_vfmacc_vv_f64m1(a1, multiplier, addend, vl);
        a2 = __riscv_vfmacc_vv_f64m1(a2, multiplier, addend, vl);
        a3 = __riscv_vfmacc_vv_f64m1(a3, multiplier, addend, vl);
        a4 = __riscv_vfmacc_vv_f64m1(a4, multiplier, addend, vl);
        a5 = __riscv_vfmacc_vv_f64m1(a5, multiplier, addend, vl);
        a6 = __riscv_vfmacc_vv_f64m1(a6, multiplier, addend, vl);
        a7 = __riscv_vfmacc_vv_f64m1(a7, multiplier, addend, vl);
    }

    const vfloat64m1_t zero = __riscv_vfmv_v_f_f64m1(0.0, vl);
    double checksum = 0.0;
#define MPERF_REDUCE(accumulator)                                             \
    checksum += __riscv_vfmv_f_s_f64m1_f64(                                  \
        __riscv_vfredusum_vs_f64m1_f64m1((accumulator), zero, vl))
    MPERF_REDUCE(a0);
    MPERF_REDUCE(a1);
    MPERF_REDUCE(a2);
    MPERF_REDUCE(a3);
    MPERF_REDUCE(a4);
    MPERF_REDUCE(a5);
    MPERF_REDUCE(a6);
    MPERF_REDUCE(a7);
#undef MPERF_REDUCE
    return checksum;
}
