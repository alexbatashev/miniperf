#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'usage: %s <extracted-qemu-bundle-directory>\n' "$0" >&2
    exit 2
fi

bundle_directory="$(realpath "$1")"
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
plugin="${repository_root}/target/release/libminiperf_qemu_roofline.so"
riscv_compiler="${RISCV_CC:-riscv64-linux-gnu-gcc}"
x86_fixture="${repository_root}/utils/qemu-roofline/tests/fixtures/x86-sse2-smoke.S"
rvv_fixture="${repository_root}/utils/qemu-roofline/tests/fixtures/rvv-smoke.S"
temporary_directory="$(mktemp -d /tmp/miniperf-qemu-verify.XXXXXX)"
trap 'rm -rf "${temporary_directory}"' EXIT

required_symbols=(
    qemu_plugin_tb_vaddr
    qemu_plugin_insn_vaddr
    qemu_plugin_insn_size
    qemu_plugin_start_code
    qemu_plugin_end_code
    qemu_plugin_entry_code
    qemu_plugin_get_registers
    qemu_plugin_read_register
)
required_counters=(
    scalar_int_ops
    scalar_float_ops
    scalar_double_ops
    vector_int_ops
    vector_float_ops
    vector_double_ops
    bytes_load
    bytes_store
    dram_bytes_load
    dram_bytes_store
    rvv_state_errors
    unclassified_instructions
)

check_qemu_api() {
    local qemu="$1"
    if [[ ! -x "${qemu}" ]]; then
        printf 'missing QEMU executable: %s\n' "${qemu}" >&2
        return 1
    fi
    local help_output
    if ! help_output="$("${qemu}" --help 2>&1)"; then
        printf 'failed to execute %s:\n%s\n' "${qemu}" "${help_output}" >&2
        return 1
    fi
    if ! grep -Eq '^[[:space:]]*-plugin' <<<"${help_output}"; then
        printf '%s does not advertise TCG plugin support\n' "${qemu}" >&2
        printf '%s\n' "${help_output}" >&2
        return 1
    fi
    local symbols
    symbols="$(readelf --dyn-syms --wide "${qemu}")"
    local symbol
    for symbol in "${required_symbols[@]}"; do
        if ! grep -Eq "[[:space:]]${symbol}(@@[^[:space:]]+)?$" <<<"${symbols}"; then
            printf '%s does not export required plugin API %s\n' "${qemu}" "${symbol}" >&2
            return 1
        fi
    done
}

validate_capture() {
    local architecture="$1"
    local counts_path="$2"
    local cfg_path="${counts_path%.*}.cfg"
    if [[ ! -s "${counts_path}" || ! -s "${cfg_path}" ]]; then
        printf '%s plugin capture did not produce counts and CFG files\n' "${architecture}" >&2
        return 1
    fi

    declare -A counters=()
    while IFS='=' read -r key value; do
        counters["${key}"]="${value}"
    done <"${counts_path}"

    local key value
    for key in "${required_counters[@]}"; do
        value="${counters[${key}]:-}"
        if [[ ! "${value}" =~ ^[0-9]+$ ]]; then
            printf '%s capture has a missing or invalid %s counter\n' "${architecture}" "${key}" >&2
            return 1
        fi
    done
    if (( counters[rvv_state_errors] != 0 || counters[unclassified_instructions] != 0 )); then
        printf '%s capture is incomplete: rvv_state_errors=%s unclassified_instructions=%s\n' \
            "${architecture}" "${counters[rvv_state_errors]}" "${counters[unclassified_instructions]}" >&2
        return 1
    fi
    local cfg_header
    cfg_header="$(head -n 1 "${cfg_path}")"
    if [[ "${cfg_header}" != 'miniperf-qemu-cfg=4' ]]; then
        printf '%s capture has an unsupported CFG format: %s\n' "${architecture}" "${cfg_header}" >&2
        return 1
    fi
    if ! grep -Eq '^cache [1-9][0-9]* [1-9][0-9]* [1-9][0-9]* write-back-write-allocate$' "${cfg_path}" || \
        ! grep -Eq '^image 0x[0-9a-f]+ 0x[0-9a-f]+ 0x[0-9a-f]+$' "${cfg_path}" || \
        ! grep -Eq '^block 0x[0-9a-f]+ 0x[0-9a-f]+ [1-9][0-9]* ' "${cfg_path}"; then
        printf '%s capture is missing image metadata or executed block ranges\n' "${architecture}" >&2
        return 1
    fi
}

for executable in readelf grep head cargo cc "${riscv_compiler}"; do
    if ! command -v "${executable}" >/dev/null; then
        printf 'verification dependency is unavailable: %s\n' "${executable}" >&2
        exit 1
    fi
done

check_qemu_api "${bundle_directory}/bin/qemu-x86_64"
check_qemu_api "${bundle_directory}/bin/qemu-riscv64"

cargo build --release -p miniperf-qemu-roofline --manifest-path "${repository_root}/Cargo.toml"

x86_counts="${temporary_directory}/x86.counts"
x86_workload="${temporary_directory}/x86-sse2-smoke"
cc -nostdlib -static "${x86_fixture}" -o "${x86_workload}"
OMP_NUM_THREADS=1 "${bundle_directory}/bin/qemu-x86_64" \
    -plugin "${plugin},output=${x86_counts}" \
    "${x86_workload}" >/dev/null
validate_capture x86_64 "${x86_counts}"
declare -A x86_counters=()
while IFS='=' read -r key value; do
    x86_counters["${key}"]="${value}"
done <"${x86_counts}"
if (( x86_counters[vector_double_ops] != 4 || x86_counters[bytes_load] != 16 || x86_counters[bytes_store] != 16 || x86_counters[dram_bytes_load] != 64 || x86_counters[dram_bytes_store] != 64 )); then
    printf 'x86_64 capture has incorrect SSE2 counts: fp64=%s load=%s store=%s dram-load=%s dram-store=%s\n' \
        "${x86_counters[vector_double_ops]}" "${x86_counters[bytes_load]}" "${x86_counters[bytes_store]}" \
        "${x86_counters[dram_bytes_load]}" "${x86_counters[dram_bytes_store]}" >&2
    exit 1
fi

rvv_counts="${temporary_directory}/rvv.counts"
rvv_workload="${temporary_directory}/rvv-smoke"
"${riscv_compiler}" \
    -nostdlib \
    -static \
    -march=rv64gcv \
    -mabi=lp64d \
    "${rvv_fixture}" \
    -o "${rvv_workload}"
OMP_NUM_THREADS=1 "${bundle_directory}/bin/qemu-riscv64" \
    -cpu rv64,v=true,vlen=256 \
    -plugin "${plugin},output=${rvv_counts}" \
    "${rvv_workload}" >/dev/null
validate_capture riscv64-rvv "${rvv_counts}"
declare -A rvv_counters=()
while IFS='=' read -r key value; do
    rvv_counters["${key}"]="${value}"
done <"${rvv_counts}"
if (( rvv_counters[vector_int_ops] != 6 || rvv_counters[vector_float_ops] != 12 || rvv_counters[vector_double_ops] != 4 )); then
    printf 'RISC-V capture has incorrect exact RVV counts: int=%s fp32=%s fp64=%s\n' \
        "${rvv_counters[vector_int_ops]}" "${rvv_counters[vector_float_ops]}" "${rvv_counters[vector_double_ops]}" >&2
    exit 1
fi
if (( rvv_counters[dram_bytes_load] != 128 || rvv_counters[dram_bytes_store] != 0 )); then
    printf 'RISC-V capture has incorrect modeled DRAM traffic: load=%s store=%s\n' \
        "${rvv_counters[dram_bytes_load]}" "${rvv_counters[dram_bytes_store]}" >&2
    exit 1
fi

printf 'QEMU Roofline bundle verification passed: plugin API plus exact x86-64 SSE2 and RISC-V RVV operation/traffic fixtures\n'
