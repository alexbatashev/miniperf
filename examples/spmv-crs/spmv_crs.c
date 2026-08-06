#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <inttypes.h>
#include <math.h>
#include <omp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#if defined(__AVX512F__) || defined(__AVX2__)
#include <immintrin.h>
#elif defined(__riscv_vector)
#include <riscv_vector.h>
#else
#error "Build this benchmark for AVX2, AVX-512, or RVV"
#endif

typedef struct {
    size_t rows;
    size_t columns;
    size_t nnz;
    size_t *row_offsets;
    uint32_t *column_indices;
    double *values;
    double *x;
    double *y;
} CrsMatrix;

static double now_seconds(void) {
    struct timespec timestamp;
    if (clock_gettime(CLOCK_MONOTONIC_RAW, &timestamp) != 0) {
        perror("clock_gettime");
        exit(EXIT_FAILURE);
    }
    return (double)timestamp.tv_sec + (double)timestamp.tv_nsec * 1.0e-9;
}

static void *allocate_aligned(size_t alignment, size_t size) {
    void *memory = NULL;
    const int error = posix_memalign(&memory, alignment, size);
    if (error != 0) {
        errno = error;
        perror("posix_memalign");
        exit(EXIT_FAILURE);
    }
    return memory;
}

static size_t parse_size(const char *text, const char *name) {
    char *end = NULL;
    errno = 0;
    const uintmax_t value = strtoumax(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || value == 0 || value > SIZE_MAX) {
        fprintf(stderr, "invalid %s: %s\n", name, text);
        exit(EXIT_FAILURE);
    }
    return (size_t)value;
}

static CrsMatrix make_matrix(size_t rows, size_t nonzeros_per_row) {
    if (rows > UINT32_MAX || nonzeros_per_row > SIZE_MAX / rows) {
        fprintf(stderr, "matrix dimensions are too large\n");
        exit(EXIT_FAILURE);
    }

    CrsMatrix matrix = {
        .rows = rows,
        .columns = rows,
        .nnz = rows * nonzeros_per_row,
    };
    matrix.row_offsets = allocate_aligned(64, (rows + 1) * sizeof(*matrix.row_offsets));
    matrix.column_indices =
        allocate_aligned(64, matrix.nnz * sizeof(*matrix.column_indices));
    matrix.values = allocate_aligned(64, matrix.nnz * sizeof(*matrix.values));
    matrix.x = allocate_aligned(64, matrix.columns * sizeof(*matrix.x));
    matrix.y = allocate_aligned(64, matrix.rows * sizeof(*matrix.y));

    for (size_t row = 0; row <= rows; ++row) {
        matrix.row_offsets[row] = row * nonzeros_per_row;
    }
    for (size_t row = 0; row < rows; ++row) {
        for (size_t lane = 0; lane < nonzeros_per_row; ++lane) {
            const size_t position = row * nonzeros_per_row + lane;
            matrix.column_indices[position] =
                (uint32_t)((row * 17 + lane * 8191) % matrix.columns);
            matrix.values[position] = 1.0 / (double)(lane + 1);
        }
    }
    for (size_t column = 0; column < matrix.columns; ++column) {
        matrix.x[column] = 0.5 + (double)(column % 97) / 97.0;
    }
    memset(matrix.y, 0, matrix.rows * sizeof(*matrix.y));
    return matrix;
}

static void free_matrix(CrsMatrix *matrix) {
    free(matrix->row_offsets);
    free(matrix->column_indices);
    free(matrix->values);
    free(matrix->x);
    free(matrix->y);
    memset(matrix, 0, sizeof(*matrix));
}

static double verify_result(const CrsMatrix *matrix) {
    double maximum_error = 0.0;
    for (size_t row = 0; row < matrix->rows; ++row) {
        double expected = 0.0;
        for (size_t position = matrix->row_offsets[row];
             position < matrix->row_offsets[row + 1];
             ++position) {
            expected +=
                matrix->values[position] * matrix->x[matrix->column_indices[position]];
        }
        const double error = fabs(expected - matrix->y[row]);
        maximum_error = fmax(maximum_error, error);
    }
    return maximum_error;
}

static int active_openmp_threads(void) {
    int threads = 1;
#pragma omp parallel shared(threads)
    {
#pragma omp single
        threads = omp_get_num_threads();
    }
    return threads;
}

#if defined(__AVX512F__)
static const char *implementation_name(void) {
    return "avx512";
}

static void spmv(const CrsMatrix *matrix) {
#pragma omp parallel for schedule(static)
    for (size_t row = 0; row < matrix->rows; ++row) {
        size_t position = matrix->row_offsets[row];
        const size_t end = matrix->row_offsets[row + 1];
        __m512d vector_sum = _mm512_setzero_pd();

        for (; position + 8 <= end; position += 8) {
            const __m512d values = _mm512_loadu_pd(&matrix->values[position]);
            const __m256i indices =
                _mm256_loadu_si256((const __m256i *)&matrix->column_indices[position]);
            const __m512d x = _mm512_i32gather_pd(indices, matrix->x, 8);
            vector_sum = _mm512_fmadd_pd(values, x, vector_sum);
        }

        double sum = _mm512_reduce_add_pd(vector_sum);
        for (; position < end; ++position) {
            sum += matrix->values[position] * matrix->x[matrix->column_indices[position]];
        }
        matrix->y[row] = sum;
    }
}
#elif defined(__AVX2__)
static const char *implementation_name(void) {
    return "avx2";
}

static void spmv(const CrsMatrix *matrix) {
#pragma omp parallel for schedule(static)
    for (size_t row = 0; row < matrix->rows; ++row) {
        size_t position = matrix->row_offsets[row];
        const size_t end = matrix->row_offsets[row + 1];
        __m256d vector_sum = _mm256_setzero_pd();

        for (; position + 4 <= end; position += 4) {
            const __m256d values = _mm256_loadu_pd(&matrix->values[position]);
            const __m128i indices =
                _mm_loadu_si128((const __m128i *)&matrix->column_indices[position]);
            const __m256d x = _mm256_i32gather_pd(matrix->x, indices, 8);
            vector_sum = _mm256_fmadd_pd(values, x, vector_sum);
        }

        const __m128d halves = _mm_add_pd(
            _mm256_castpd256_pd128(vector_sum),
            _mm256_extractf128_pd(vector_sum, 1));
        double sum = _mm_cvtsd_f64(_mm_hadd_pd(halves, halves));
        for (; position < end; ++position) {
            sum += matrix->values[position] * matrix->x[matrix->column_indices[position]];
        }
        matrix->y[row] = sum;
    }
}
#elif defined(__riscv_vector)
static const char *implementation_name(void) {
    return "rvv";
}

static void spmv(const CrsMatrix *matrix) {
#pragma omp parallel for schedule(static)
    for (size_t row = 0; row < matrix->rows; ++row) {
        size_t position = matrix->row_offsets[row];
        const size_t end = matrix->row_offsets[row + 1];
        double sum = 0.0;

        while (position < end) {
            const size_t vl = __riscv_vsetvl_e64m1(end - position);
            const vfloat64m1_t values = __riscv_vle64_v_f64m1(&matrix->values[position], vl);
            vuint32mf2_t offsets =
                __riscv_vle32_v_u32mf2(&matrix->column_indices[position], vl);
            offsets = __riscv_vsll_vx_u32mf2(offsets, 3, vl);
            const vfloat64m1_t x = __riscv_vluxei32_v_f64m1(matrix->x, offsets, vl);
            const vfloat64m1_t products = __riscv_vfmul_vv_f64m1(values, x, vl);
            const vfloat64m1_t initial = __riscv_vfmv_v_f_f64m1(0.0, 1);
            const vfloat64m1_t reduced =
                __riscv_vfredusum_vs_f64m1_f64m1(products, initial, vl);
            sum += __riscv_vfmv_f_s_f64m1_f64(reduced);
            position += vl;
        }
        matrix->y[row] = sum;
    }
}
#endif

int main(int argc, char **argv) {
    const size_t rows = argc > 1 ? parse_size(argv[1], "rows") : 262144;
    const size_t nonzeros_per_row =
        argc > 2 ? parse_size(argv[2], "nonzeros per row") : 16;
    const size_t repetitions = argc > 3 ? parse_size(argv[3], "repetitions") : 40;
    if (argc > 4) {
        fprintf(stderr, "usage: %s [rows [nonzeros-per-row [repetitions]]]\n", argv[0]);
        return EXIT_FAILURE;
    }

    CrsMatrix matrix = make_matrix(rows, nonzeros_per_row);
    for (size_t warmup = 0; warmup < 3; ++warmup) {
        spmv(&matrix);
    }
    const double maximum_error = verify_result(&matrix);
    if (maximum_error > 1.0e-11) {
        fprintf(stderr, "SpMV verification failed: maximum error %.17g\n", maximum_error);
        free_matrix(&matrix);
        return EXIT_FAILURE;
    }

    const double start = now_seconds();
    for (size_t repetition = 0; repetition < repetitions; ++repetition) {
        spmv(&matrix);
    }
    const double seconds = now_seconds() - start;

    double checksum = 0.0;
    for (size_t row = 0; row < matrix.rows; ++row) {
        checksum += matrix.y[row];
    }

    const double operations = 2.0 * (double)matrix.nnz * (double)repetitions;
    const double bytes_per_spmv =
        20.0 * (double)matrix.nnz + 16.0 * (double)matrix.rows + 8.0;
    const double arithmetic_intensity = 2.0 * (double)matrix.nnz / bytes_per_spmv;

    printf("implementation=%s\n", implementation_name());
    printf("openmp_threads=%d\n", active_openmp_threads());
    printf("rows=%zu\n", matrix.rows);
    printf("columns=%zu\n", matrix.columns);
    printf("nonzeros_per_row=%zu\n", nonzeros_per_row);
    printf("nonzeros=%zu\n", matrix.nnz);
    printf("repetitions=%zu\n", repetitions);
    printf("seconds=%.9f\n", seconds);
    printf("gflops=%.6f\n", operations / seconds / 1.0e9);
    printf("algorithmic_bytes_per_spmv=%.0f\n", bytes_per_spmv);
    printf("arithmetic_intensity=%.9f\n", arithmetic_intensity);
    printf("effective_bandwidth_gbs=%.6f\n",
           bytes_per_spmv * (double)repetitions / seconds / 1.0e9);
    printf("checksum=%.17g\n", checksum);
    printf("maximum_abs_error=%.17g\n", maximum_error);

    free_matrix(&matrix);
    return EXIT_SUCCESS;
}
