#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static int output_fd = -1;
static __thread int inside_hook;
static __thread int child_reported;
static pid_t root_pid;
extern void *__libc_malloc(size_t) __attribute__((weak));
extern void *__libc_calloc(size_t, size_t) __attribute__((weak));
extern void *__libc_realloc(void *, size_t) __attribute__((weak));
extern void __libc_free(void *) __attribute__((weak));

static void *resolve_next(const char *name) {
    int previous = inside_hook;
    inside_hook = 1;
    void *symbol = dlsym(RTLD_NEXT, name);
    inside_hook = previous;
    return symbol;
}

static uint64_t timestamp_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) return 0;
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

static char *append_u64(char *out, uint64_t value, int hex) {
    char digits[32];
    unsigned count = 0;
    unsigned base = hex ? 16 : 10;
    do {
        unsigned digit = value % base;
        digits[count++] = (char)(digit < 10 ? '0' + digit : 'a' + digit - 10);
        value /= base;
    } while (value);
    while (count) *out++ = digits[--count];
    return out;
}

static void emit(char operation, uintptr_t first, uintptr_t second, size_t size) {
    if (output_fd < 0 || inside_hook) return;
    pid_t current_pid = getpid();
    if (root_pid && current_pid != root_pid) {
        if (child_reported) return;
        child_reported = 1;
        operation = 'C'; first = (uintptr_t)current_pid; second = 0; size = 0;
    }
    inside_hook = 1;
    char line[160], *out = line;
    *out++ = operation; *out++ = ' ';
    out = append_u64(out, timestamp_ns(), 0); *out++ = ' ';
    out = append_u64(out, first, 16); *out++ = ' ';
    out = append_u64(out, second, 16); *out++ = ' ';
    out = append_u64(out, size, 0); *out++ = '\n';
    (void)syscall(SYS_write, output_fd, line, (size_t)(out - line));
    inside_hook = 0;
}

__attribute__((constructor)) static void initialize(void) {
    const char *path = getenv("MPERF_MEMORY_ALLOCATIONS");
    const char *root = getenv("MPERF_PROFILE_ROOT_PID");
    if (root) root_pid = (pid_t)strtol(root, NULL, 10);
    if (path) output_fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_APPEND, 0600);
}

__attribute__((destructor)) static void finish(void) {
    if (output_fd >= 0) close(output_fd);
}

void *malloc(size_t size) {
    static void *(*next)(size_t);
    if (inside_hook && !next && __libc_malloc) return __libc_malloc(size);
    if (!next) next = resolve_next("malloc");
    void *pointer = next(size);
    emit('A', (uintptr_t)pointer, 0, size);
    return pointer;
}

void *calloc(size_t count, size_t size) {
    static void *(*next)(size_t, size_t);
    if (inside_hook && !next && __libc_calloc) return __libc_calloc(count, size);
    if (!next) next = resolve_next("calloc");
    void *pointer = next(count, size);
    emit('A', (uintptr_t)pointer, 0, count * size);
    return pointer;
}

void free(void *pointer) {
    static void (*next)(void *);
    if (inside_hook && !next && __libc_free) { __libc_free(pointer); return; }
    if (!next) next = resolve_next("free");
    emit('F', (uintptr_t)pointer, 0, 0);
    next(pointer);
}

void *realloc(void *pointer, size_t size) {
    static void *(*next)(void *, size_t);
    if (inside_hook && !next && __libc_realloc) return __libc_realloc(pointer, size);
    if (!next) next = resolve_next("realloc");
    void *replacement = next(pointer, size);
    emit('R', (uintptr_t)pointer, (uintptr_t)replacement, size);
    return replacement;
}

void *mmap(void *address, size_t length, int prot, int flags, int fd, off_t offset) {
    static void *(*next)(void *, size_t, int, int, int, off_t);
    if (inside_hook && !next) return (void *)syscall(SYS_mmap, address, length, prot, flags, fd, offset);
    if (!next) next = resolve_next("mmap");
    void *result = next(address, length, prot, flags, fd, offset);
    if (result != MAP_FAILED && (flags & MAP_ANONYMOUS)) emit('M', (uintptr_t)result, 0, length);
    return result;
}

int munmap(void *address, size_t length) {
    static int (*next)(void *, size_t);
    if (inside_hook && !next) return (int)syscall(SYS_munmap, address, length);
    if (!next) next = resolve_next("munmap");
    int result = next(address, length);
    if (result == 0) emit('U', (uintptr_t)address, 0, length);
    return result;
}

void *aligned_alloc(size_t alignment, size_t size) {
    static void *(*next)(size_t, size_t);
    if (!next) next = resolve_next("aligned_alloc");
    void *pointer = next(alignment, size);
    emit('A', (uintptr_t)pointer, 0, size);
    return pointer;
}

int posix_memalign(void **pointer, size_t alignment, size_t size) {
    static int (*next)(void **, size_t, size_t);
    if (!next) next = resolve_next("posix_memalign");
    int result = next(pointer, alignment, size);
    if (result == 0) emit('A', (uintptr_t)*pointer, 0, size);
    return result;
}
