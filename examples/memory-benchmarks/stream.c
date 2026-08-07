#include "bench.h"
#ifdef MINIPERF_MANUAL_ROOFLINE
#include "roofline.h"
#endif

int main(int argc, char **argv) {
    const size_t n = bench_size_arg(argc, argv, 1, 1u << 24, "element count");
    const size_t passes = bench_size_arg(argc, argv, 2, 1000, "pass count");
    if (n > SIZE_MAX / sizeof(double)) {
        fputs("array is too large\n", stderr);
        return EXIT_FAILURE;
    }

    const size_t bytes = n * sizeof(double);
    double *restrict a = bench_alloc(bytes);
    double *restrict b = bench_alloc(bytes);
    double *restrict c = bench_alloc(bytes);
    for (size_t i = 0; i < n; ++i) {
        a[i] = 0.0;
        b[i] = 1.0 + (double)(i & 7u);
        c[i] = 2.0 - (double)(i & 3u) * 0.125;
    }

    const double start = bench_seconds();
#ifdef MINIPERF_MANUAL_ROOFLINE
    void *roofline = bench_roofline_begin(__LINE__ + 1, __FILE__);
#endif
    for (size_t p = 0; p < passes; ++p) {
        const double scalar = 0.5 + (double)p * 0.03125;
        for (size_t i = 0; i < n; ++i) {
            a[i] = b[i] + scalar * c[i];
        }
        double *tmp = a;
        a = b;
        b = tmp;
    }
#ifdef MINIPERF_MANUAL_ROOFLINE
    const uint64_t updates = (uint64_t)n * passes;
    const struct mperf_loop_stats roofline_stats = {
        .trip_count = updates,
        .bytes_load = updates * 16,
        .bytes_store = updates * 8,
        .vector_double_ops = updates * 2,
    };
    bench_roofline_end(roofline, &roofline_stats);
#endif
    const double elapsed = bench_seconds() - start;

    double checksum = 0.0;
    for (size_t i = 0; i < n; ++i) {
        checksum += b[i];
    }
    const double traffic = 3.0 * (double)bytes * (double)passes;
    printf("stream elements=%zu passes=%zu seconds=%.6f modeled_GB/s=%.3f "
           "checksum=%.9e\n",
           n, passes, elapsed, traffic / elapsed / 1.0e9, checksum);

    free(c);
    free(b);
    free(a);
    return checksum == 0.0 ? EXIT_FAILURE : EXIT_SUCCESS;
}
