#include "bench.h"
#if defined(__AVX2__) && defined(__FMA__)
#include <immintrin.h>
#endif
#ifdef MINIPERF_MANUAL_ROOFLINE
#include "roofline.h"
#endif

int main(int argc, char **argv) {
    const size_t n = bench_size_arg(argc, argv, 1, 512, "matrix size");
    const size_t repeats = bench_size_arg(argc, argv, 2, 1100, "repeat count");
    if (n > SIZE_MAX / n || n * n > SIZE_MAX / sizeof(double)) {
        fputs("matrix is too large\n", stderr);
        return EXIT_FAILURE;
    }

    const size_t elements = n * n;
    double *restrict a = bench_alloc(elements * sizeof(*a));
    double *restrict b = bench_alloc(elements * sizeof(*b));
    double *restrict c = bench_alloc(elements * sizeof(*c));
    for (size_t i = 0; i < elements; ++i) {
        a[i] = (double)((i * 17u) % 101u) / 101.0;
        b[i] = (double)((i * 29u) % 103u) / 103.0;
        c[i] = 0.0;
    }

    const double start = bench_seconds();
#ifdef MINIPERF_MANUAL_ROOFLINE
    void *roofline = bench_roofline_begin(__LINE__ + 1, __FILE__);
#endif
    for (size_t r = 0; r < repeats; ++r) {
        for (size_t i = 0; i < n; ++i) {
            for (size_t k = 0; k < n; ++k) {
                const double aik = a[i * n + k];
#if defined(__AVX2__) && defined(__FMA__)
                const __m256d va = _mm256_set1_pd(aik);
                size_t j = 0;
                for (; j + 15 < n; j += 16) {
                    __m256d c0 = _mm256_loadu_pd(&c[i * n + j]);
                    __m256d c1 = _mm256_loadu_pd(&c[i * n + j + 4]);
                    __m256d c2 = _mm256_loadu_pd(&c[i * n + j + 8]);
                    __m256d c3 = _mm256_loadu_pd(&c[i * n + j + 12]);
                    c0 = _mm256_fmadd_pd(va, _mm256_loadu_pd(&b[k * n + j]), c0);
                    c1 = _mm256_fmadd_pd(va, _mm256_loadu_pd(&b[k * n + j + 4]), c1);
                    c2 = _mm256_fmadd_pd(va, _mm256_loadu_pd(&b[k * n + j + 8]), c2);
                    c3 = _mm256_fmadd_pd(va, _mm256_loadu_pd(&b[k * n + j + 12]), c3);
                    _mm256_storeu_pd(&c[i * n + j], c0);
                    _mm256_storeu_pd(&c[i * n + j + 4], c1);
                    _mm256_storeu_pd(&c[i * n + j + 8], c2);
                    _mm256_storeu_pd(&c[i * n + j + 12], c3);
                }
                for (; j + 3 < n; j += 4) {
                    __m256d cv = _mm256_loadu_pd(&c[i * n + j]);
                    cv = _mm256_fmadd_pd(va, _mm256_loadu_pd(&b[k * n + j]), cv);
                    _mm256_storeu_pd(&c[i * n + j], cv);
                }
                for (; j < n; ++j) {
                    c[i * n + j] += aik * b[k * n + j];
                }
#else
                for (size_t j = 0; j < n; ++j) {
                    c[i * n + j] += aik * b[k * n + j];
                }
#endif
            }
        }
    }
#ifdef MINIPERF_MANUAL_ROOFLINE
    const uint64_t updates = (uint64_t)n * n * n * repeats;
    const struct mperf_loop_stats roofline_stats = {
        .trip_count = updates,
        .bytes_load = (uint64_t)n * n * 16,
        .bytes_store = (uint64_t)n * n * 8,
        .vector_double_ops = updates * 2,
    };
    bench_roofline_end(roofline, &roofline_stats);
#endif
    const double elapsed = bench_seconds() - start;

    double checksum = 0.0;
    for (size_t i = 0; i < elements; ++i) {
        checksum += c[i];
    }
    const double operations = 2.0 * (double)n * (double)n * (double)n *
                              (double)repeats;
    printf("matmul n=%zu repeats=%zu seconds=%.6f gflop/s=%.3f checksum=%.9e\n",
           n, repeats, elapsed, operations / elapsed / 1.0e9, checksum);

    free(c);
    free(b);
    free(a);
    return checksum == 0.0 ? EXIT_FAILURE : EXIT_SUCCESS;
}
