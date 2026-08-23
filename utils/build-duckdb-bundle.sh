#!/usr/bin/env bash
# Builds a prebuilt DuckDB for miniperf-store to link against, replacing the
# duckdb crate's `bundled` feature (which recompiles DuckDB from source on
# every clean build).
#
# The archives CMake produces — the extension loader, core_functions, parquet,
# jemalloc where enabled, and duckdb_static itself — are merged into a single
# libduckdb_static.a so that `DUCKDB_STATIC=1 DUCKDB_LIB_DIR=<bundle>/lib` is
# all libduckdb-sys needs. Link order stops mattering once there is one archive.
#
# usage: build-duckdb-bundle.sh <platform> [output-directory]
set -euo pipefail

# shellcheck source=utils/deps/common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/deps/common.sh"

platform="${1:?usage: build-duckdb-bundle.sh <platform> [output-directory]}"
output_directory="${2:-dist}"
version="${DUCKDB_VERSION:-$(deps_manifest_get upstream.duckdb.version)}"
repository="${DUCKDB_REPOSITORY:-$(deps_manifest_get upstream.duckdb.repository)}"
bundle_name="miniperf-duckdb-${version}-${platform}"

build_root="$(mktemp -d "${DEPS_BUILD_PARENT:-/tmp}/miniperf-duckdb.XXXXXX")"
trap 'rm -rf "${build_root}"' EXIT
source_directory="${build_root}/source"
build_directory="${build_root}/build"
install_directory="${build_root}/install"
bundle_directory="${build_root}/${bundle_name}"

git clone --depth 1 --branch "v${version}" "${repository}" "${source_directory}"

cmake_arguments=(
    -S "${source_directory}"
    -B "${build_directory}"
    -G Ninja
    -DCMAKE_BUILD_TYPE=Release
    -DCMAKE_INSTALL_LIBDIR=lib
    -DBUILD_UNITTESTS=0
    -DBUILD_SHELL=0
    -DBUILD_SHARED_LIBS=0
    -DDISABLE_UNITY=0
    -DDISABLE_EXTENSION_LOAD=0
    -DENABLE_EXTENSION_AUTOLOADING=1
    -DENABLE_EXTENSION_AUTOINSTALL=1
    -DBUILD_EXTENSIONS=parquet
    -DSKIP_EXTENSIONS=
)

archiver=ar
warning_flag=-w
case "${platform}" in
    linux-x86_64 | linux-aarch64)
        # Matches what the duckdb crate's bundled build enables.
        cmake_arguments+=(-DENABLE_JEMALLOC=ON)
        ;;
    linux-riscv64)
        toolchain="${build_root}/riscv64.cmake"
        cat >"${toolchain}" <<'EOF'
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR riscv64)
set(CMAKE_C_COMPILER riscv64-linux-gnu-gcc)
set(CMAKE_CXX_COMPILER riscv64-linux-gnu-g++)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF
        cmake_arguments+=(
            "-DCMAKE_TOOLCHAIN_FILE=${toolchain}"
            "-DCMAKE_C_FLAGS=-march=${RISCV_MARCH:-rv64gcv_zba_zbb} -mabi=lp64d"
            "-DCMAKE_CXX_FLAGS=-march=${RISCV_MARCH:-rv64gcv_zba_zbb} -mabi=lp64d"
            -DENABLE_JEMALLOC=OFF
        )
        archiver=riscv64-linux-gnu-ar
        ;;
    macos-aarch64)
        cmake_arguments+=(
            -DCMAKE_OSX_ARCHITECTURES=arm64
            "-DCMAKE_OSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}"
        )
        ;;
    windows-x86_64)
        warning_flag=/w
        ;;
    *)
        printf 'unsupported platform for DuckDB: %s\n' "${platform}" >&2
        exit 2
        ;;
esac

cmake_arguments+=(
    "-DCMAKE_C_FLAGS_INIT=${warning_flag}"
    "-DCMAKE_CXX_FLAGS_INIT=${warning_flag}"
)

cmake "${cmake_arguments[@]}"
cmake --build "${build_directory}"
cmake --install "${build_directory}" --prefix "${install_directory}"

mkdir -p "${bundle_directory}/lib" "${bundle_directory}/include"
cp "${install_directory}/include/duckdb.h" "${bundle_directory}/include/duckdb.h"
cp "${install_directory}/include/duckdb.hpp" "${bundle_directory}/include/duckdb.hpp"
cp "${source_directory}/LICENSE" "${bundle_directory}/DUCKDB_LICENSE.txt"

if [[ "${platform}" == windows-* ]]; then
    mapfile -t component_libraries < <(find "${install_directory}/lib" -name '*.lib' | sort)
else
    mapfile -t component_libraries < <(find "${install_directory}/lib" -name 'lib*.a' | sort)
fi
if [[ "${#component_libraries[@]}" -eq 0 ]]; then
    printf 'DuckDB install produced no static libraries in %s\n' "${install_directory}/lib" >&2
    exit 1
fi
printf 'Merging %d DuckDB archives\n' "${#component_libraries[@]}"

case "${platform}" in
    windows-*)
        merged_library="${bundle_directory}/lib/duckdb_static.lib"
        lib.exe "/OUT:${merged_library}" "${component_libraries[@]}"
        ;;
    macos-*)
        merged_library="${bundle_directory}/lib/libduckdb_static.a"
        libtool -static -o "${merged_library}" "${component_libraries[@]}"
        ;;
    *)
        merged_library="${bundle_directory}/lib/libduckdb_static.a"
        {
            printf 'create %s\n' "${merged_library}"
            printf 'addlib %s\n' "${component_libraries[@]}"
            printf 'save\nend\n'
        } | "${archiver}" -M
        ;;
esac

{
    printf 'duckdb_version=%s\n' "${version}"
    printf 'platform=%s\n' "${platform}"
    printf 'extensions=parquet,core_functions\n'
    printf 'merged_archives=%s\n' "$(basename -a "${component_libraries[@]}" | paste -sd, -)"
} >"${bundle_directory}/MANIFEST.txt"

# Prove the merged archive is self-contained and that parquet is compiled in
# rather than autoloaded, which is the whole reason miniperf-store links it.
smoke_source="${build_root}/duckdb-smoke.c"
cat >"${smoke_source}" <<'EOF'
#include <duckdb.h>
#include <stdint.h>
#include <stdio.h>

int main(void) {
    duckdb_database database;
    duckdb_connection connection;
    duckdb_result result;

    if (duckdb_open(NULL, &database) != DuckDBSuccess) return 1;
    if (duckdb_connect(database, &connection) != DuckDBSuccess) return 2;
    if (duckdb_query(connection,
                     "COPY (SELECT 42::BIGINT AS answer) TO 'smoke.parquet' (FORMAT PARQUET)",
                     NULL) != DuckDBSuccess)
        return 3;
    if (duckdb_query(connection, "SELECT answer FROM read_parquet('smoke.parquet')", &result) !=
        DuckDBSuccess)
        return 4;

    duckdb_data_chunk chunk = duckdb_fetch_chunk(result);
    if (chunk == NULL) return 5;
    int64_t *values = (int64_t *)duckdb_vector_get_data(duckdb_data_chunk_get_vector(chunk, 0));
    if (values[0] != 42) return 6;

    printf("duckdb %s parquet round-trip ok\n", duckdb_library_version());
    return 0;
}
EOF

smoke_binary="${build_root}/duckdb-smoke"
case "${platform}" in
    windows-*)
        smoke_binary="${build_root}/duckdb-smoke.exe"
        cl.exe /nologo "/I${bundle_directory}/include" "${smoke_source}" \
            "/Fe:${smoke_binary}" "${merged_library}" ws2_32.lib rstrtmgr.lib bcrypt.lib
        ;;
    macos-*)
        cc -I"${bundle_directory}/include" "${smoke_source}" -o "${smoke_binary}" \
            "${merged_library}" -lc++
        ;;
    linux-riscv64)
        riscv64-linux-gnu-gcc -static -I"${bundle_directory}/include" "${smoke_source}" \
            -o "${smoke_binary}" "${merged_library}" -lstdc++ -lpthread -ldl -lm
        ;;
    *)
        cc -I"${bundle_directory}/include" "${smoke_source}" -o "${smoke_binary}" \
            "${merged_library}" -lstdc++ -lpthread -ldl -lm
        ;;
esac

smoke_runner=()
if [[ "${platform}" == linux-riscv64 ]]; then
    # Cross-built on x86; run the check under qemu-user when the host has it.
    if command -v qemu-riscv64-static >/dev/null 2>&1; then
        smoke_runner=(qemu-riscv64-static)
    else
        printf 'qemu-riscv64-static is unavailable; DuckDB bundle verified by linking only\n' >&2
    fi
fi
if [[ "${platform}" != linux-riscv64 || "${#smoke_runner[@]}" -gt 0 ]]; then
    (cd "${build_root}" && "${smoke_runner[@]}" "${smoke_binary}")
fi

deps_publish duckdb "${platform}" "${version}" "${build_root}" "${bundle_name}" "${output_directory}"
