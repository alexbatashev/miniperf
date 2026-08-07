/* RVV saxpy test for the DR prototype: known flop count.
 * y[i] += a * x[i], n elements, passes iterations, float32.
 * Expected vector_float ops = 2 * n * passes (vfmacc counts as 2).
 */
#include <riscv_vector.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    size_t n = argc > 1 ? strtoul(argv[1], NULL, 10) : 100000;
    size_t passes = argc > 2 ? strtoul(argv[2], NULL, 10) : 100;
    float *x = malloc(n * sizeof(float));
    float *y = malloc(n * sizeof(float));
    for (size_t i = 0; i < n; i++) {
        x[i] = (float)(i & 15) * 0.25f;
        y[i] = 1.0f;
    }
    float a = 1.00001f;
    for (size_t p = 0; p < passes; p++) {
        size_t i = 0;
        while (i < n) {
            size_t vl = __riscv_vsetvl_e32m4(n - i);
            vfloat32m4_t vx = __riscv_vle32_v_f32m4(&x[i], vl);
            vfloat32m4_t vy = __riscv_vle32_v_f32m4(&y[i], vl);
            vy = __riscv_vfmacc_vf_f32m4(vy, a, vx, vl);
            __riscv_vse32_v_f32m4(&y[i], vy, vl);
            i += vl;
        }
    }
    double sum = 0;
    for (size_t i = 0; i < n; i++)
        sum += y[i];
    printf("n=%zu passes=%zu checksum=%.6e expected_vf_ops=%zu\n", n, passes,
           sum, 2 * n * passes);
    free(x);
    free(y);
    return 0;
}
