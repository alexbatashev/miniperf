#include <stdint.h>
#include <stdio.h>
#include <time.h>

__attribute__((noinline)) static uint64_t leaf_work(uint64_t value) {
    for (unsigned i = 0; i < 20000; ++i) {
        value = value * 6364136223846793005ULL + 1442695040888963407ULL;
        value ^= value >> 17;
    }
    return value;
}

__attribute__((noinline)) static uint64_t middle_work(uint64_t value) {
    return leaf_work(value) ^ leaf_work(value + 1);
}

int main(void) {
    struct timespec start;
    struct timespec now;
    uint64_t value = 1;

    clock_gettime(CLOCK_MONOTONIC, &start);
    do {
        value = middle_work(value);
        clock_gettime(CLOCK_MONOTONIC, &now);
    } while ((now.tv_sec - start.tv_sec) * 1000000000LL +
                 (now.tv_nsec - start.tv_nsec) <
             2000000000LL);

    printf("smoke checksum: %llu\n", (unsigned long long)value);
    return 0;
}
