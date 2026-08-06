#ifndef MINIPERF_MEMORY_BENCH_H
#define MINIPERF_MEMORY_BENCH_H

#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static inline double bench_seconds(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        perror("clock_gettime");
        exit(EXIT_FAILURE);
    }
    return (double)ts.tv_sec + (double)ts.tv_nsec * 1.0e-9;
}

static inline size_t bench_size_arg(int argc, char **argv, int index,
                                    size_t fallback, const char *name) {
    if (argc <= index) {
        return fallback;
    }
    errno = 0;
    char *end = NULL;
    uintmax_t value = strtoumax(argv[index], &end, 10);
    if (errno != 0 || end == argv[index] || *end != '\0' || value == 0 ||
        value > SIZE_MAX) {
        fprintf(stderr, "invalid %s: %s\n", name, argv[index]);
        exit(EXIT_FAILURE);
    }
    return (size_t)value;
}

static inline void *bench_alloc(size_t bytes) {
    void *ptr = NULL;
    if (posix_memalign(&ptr, 64, bytes) != 0) {
        fprintf(stderr, "could not allocate %zu bytes\n", bytes);
        exit(EXIT_FAILURE);
    }
    return ptr;
}

#endif
