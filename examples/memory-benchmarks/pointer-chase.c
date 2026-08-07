#include "bench.h"

int main(int argc, char **argv) {
    const size_t n = bench_size_arg(argc, argv, 1, 1u << 24, "node count");
    const size_t laps = bench_size_arg(argc, argv, 2, 10, "lap count");
    if ((n & (n - 1)) != 0) {
        fputs("node count must be a power of two\n", stderr);
        return EXIT_FAILURE;
    }
    if (n > SIZE_MAX / sizeof(size_t)) {
        fputs("list is too large\n", stderr);
        return EXIT_FAILURE;
    }

    size_t *next = bench_alloc(n * sizeof(*next));
    const size_t mask = n - 1;
    const size_t multiplier = 5;
    const size_t increment = 1;
    size_t current = 0;
    for (size_t i = 0; i < n; ++i) {
        const size_t successor = (current * multiplier + increment) & mask;
        next[current] = successor;
        current = successor;
    }

    current = 0;
    const double start = bench_seconds();
    for (size_t lap = 0; lap < laps; ++lap) {
        for (size_t i = 0; i < n; ++i) {
            current = next[current];
        }
    }
    const double elapsed = bench_seconds() - start;

    printf("pointer-chase nodes=%zu laps=%zu seconds=%.6f ns/access=%.3f "
           "final=%zu\n",
           n, laps, elapsed, elapsed * 1.0e9 / ((double)n * (double)laps),
           current);
    free(next);
    return current == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
}
