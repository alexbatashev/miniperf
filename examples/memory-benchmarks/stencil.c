#include "bench.h"
#ifdef MINIPERF_MANUAL_ROOFLINE
#include "roofline.h"
#endif

int main(int argc, char **argv) {
    const size_t side = bench_size_arg(argc, argv, 1, 4096, "grid side");
    const size_t steps = bench_size_arg(argc, argv, 2, 1500, "step count");
    if (side < 3 || side > SIZE_MAX / side ||
        side * side > SIZE_MAX / sizeof(double)) {
        fputs("grid size is invalid\n", stderr);
        return EXIT_FAILURE;
    }

    const size_t elements = side * side;
    const size_t bytes = elements * sizeof(double);
    double *restrict source = bench_alloc(bytes);
    double *restrict target = bench_alloc(bytes);
    for (size_t i = 0; i < elements; ++i) {
        source[i] = (double)((i * 13u) & 255u) / 255.0;
        target[i] = 0.0;
    }

    const double start = bench_seconds();
#ifdef MINIPERF_MANUAL_ROOFLINE
    void *roofline = bench_roofline_begin(__LINE__ + 1, __FILE__);
#endif
    for (size_t step = 0; step < steps; ++step) {
        for (size_t row = 1; row + 1 < side; ++row) {
            for (size_t col = 1; col + 1 < side; ++col) {
                const size_t at = row * side + col;
                target[at] = 0.2 * (source[at] + source[at - 1] +
                                    source[at + 1] + source[at - side] +
                                    source[at + side]);
            }
        }
        double *tmp = source;
        source = target;
        target = tmp;
    }
#ifdef MINIPERF_MANUAL_ROOFLINE
    const uint64_t instrumented_updates =
        (uint64_t)(side - 2) * (side - 2) * steps;
    const struct mperf_loop_stats roofline_stats = {
        .trip_count = instrumented_updates,
        .bytes_load = instrumented_updates * 40,
        .bytes_store = instrumented_updates * 8,
        .vector_double_ops = instrumented_updates * 5,
    };
    bench_roofline_end(roofline, &roofline_stats);
#endif
    const double elapsed = bench_seconds() - start;

    double checksum = 0.0;
    for (size_t i = 0; i < elements; ++i) {
        checksum += source[i];
    }
    const double updates = (double)(side - 2) * (double)(side - 2) *
                           (double)steps;
    const double traffic = updates * 6.0 * sizeof(double);
    printf("stencil side=%zu steps=%zu seconds=%.6f modeled_GB/s=%.3f "
           "checksum=%.9e\n",
           side, steps, elapsed, traffic / elapsed / 1.0e9, checksum);

    free(target);
    free(source);
    return checksum == 0.0 ? EXIT_FAILURE : EXIT_SUCCESS;
}
