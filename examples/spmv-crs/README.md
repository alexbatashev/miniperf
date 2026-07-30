# CRS SpMV example

This example implements the same deterministic CRS sparse matrix-vector
multiplication with AVX-512 and RISC-V Vector kernels. OpenMP partitions rows
statically across the worker team.

- AVX-512 uses eight-wide FP64 loads, gathers, FMA, and horizontal reduction.
- RVV selects `vl` at runtime and uses indexed FP64 gathers, vector multiply,
  and an unordered reduction.

Build both executables from the repository root:

```sh
make -C examples/spmv-crs
```

The generated executables are placed in `examples/spmv-crs/build/`. Build and
measurement directories are ignored by Git.

The default matrix has 262,144 rows and 16 nonzeros per row. The reported
algorithmic byte count includes each FP64 value, 32-bit column index, and FP64
`x` element, plus CRS row offsets and the output vector. Cache reuse can make
the physical DRAM traffic lower than this algorithmic model.

Run the native AVX-512 executable on four CPUs:

```sh
OMP_NUM_THREADS=4 OMP_PLACES=cores OMP_PROC_BIND=close \
  taskset -c 0-3 examples/spmv-crs/build/spmv-avx512
```

The executable reports its OpenMP team size, elapsed time, arithmetic
intensity, throughput, and validation checksum.

Run the RVV executable with a matching Linux sysroot and a vector-capable QEMU:

```sh
qemu-riscv64 -L /usr/riscv64-linux-gnu \
  -cpu rv64,v=true,vlen=256 \
  examples/spmv-crs/build/spmv-rvv
```

See the [Roofline analysis tutorial](../../docs/tutorials/roofline.md) for
recording commands and guidance on interpreting the results.
