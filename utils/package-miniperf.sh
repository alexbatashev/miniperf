#!/usr/bin/env bash
# Build a relocatable miniperf package for one Rust target.
set -euo pipefail

target="${1:?usage: package-miniperf.sh <rust-target> [output-directory]}"
output_directory="${2:-dist}"
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(awk -F '"' '/^version = / { print $2; exit }' "${repository_root}/mperf/Cargo.toml")"
package_name="miniperf-${version}-${target}"

mkdir -p "${output_directory}"
output_directory="$(cd "${output_directory}" && pwd)"
target_directory="${CARGO_TARGET_DIR:-${repository_root}/target}"
mkdir -p "${target_directory}"
target_directory="$(cd "${target_directory}" && pwd)"
staging_root="$(mktemp -d "${TMPDIR:-/tmp}/miniperf-package.XXXXXX")"
trap 'rm -rf "${staging_root}"' EXIT
package_root="${staging_root}/${package_name}"

package_crates=(-p mperf -p collector)
if [[ "${target}" != riscv64gc-unknown-linux-gnu ]]; then
    package_crates+=(-p mperf-gui)
fi

cargo build \
    --locked \
    --release \
    --target "${target}" \
    --manifest-path "${repository_root}/Cargo.toml" \
    "${package_crates[@]}"

mkdir -p \
    "${package_root}/bin" \
    "${package_root}/lib/miniperf" \
    "${package_root}/share/doc/miniperf"
install -m 0755 "${target_directory}/${target}/release/mperf" "${package_root}/bin/mperf"
install -m 0644 "${repository_root}/README.md" "${package_root}/share/doc/miniperf/README.md"
install -m 0644 "${repository_root}/LICENSE" "${package_root}/share/doc/miniperf/LICENSE"

if [[ "${target}" == *-linux-* ]]; then
    install -m 0755 \
        "${target_directory}/${target}/release/libcollector.so" \
        "${package_root}/lib/miniperf/libcollector.so"
    cargo build \
        --locked \
        --release \
        --target "${target}" \
        --manifest-path "${repository_root}/Cargo.toml" \
        -p miniperf-qemu-roofline
    install -m 0755 \
        "${target_directory}/${target}/release/libminiperf_qemu_roofline.so" \
        "${package_root}/lib/miniperf/libminiperf_qemu_roofline.so"

    preload_library="$(find \
        "${target_directory}/${target}/release/build" \
        -path '*/mperf-*/out/libmperf_memory_preload.so' \
        -print -quit)"
    if [[ -z "${preload_library}" ]]; then
        printf 'Memory preload library for %s was not built\n' "${target}" >&2
        exit 1
    fi
    install -m 0755 \
        "${preload_library}" \
        "${package_root}/lib/miniperf/libmperf_memory_preload.so"
else
    install -m 0755 \
        "${target_directory}/${target}/release/libcollector.dylib" \
        "${package_root}/lib/miniperf/libcollector.dylib"
fi

if [[ "${target}" != riscv64gc-unknown-linux-gnu ]]; then
    install -m 0755 \
        "${target_directory}/${target}/release/mperf-gui" \
        "${package_root}/bin/mperf-gui"
fi

{
    printf 'name=%s\n' "${package_name}"
    printf 'version=%s\n' "${version}"
    printf 'target=%s\n' "${target}"
    printf 'commit=%s\n' "$(git -C "${repository_root}" rev-parse HEAD)"
} >"${package_root}/MANIFEST.txt"

archive="${output_directory}/${package_name}.tar.gz"
tar --create --gzip --file "${archive}" --directory "${staging_root}" "${package_name}"
if command -v sha256sum >/dev/null 2>&1; then
    archive_sha256="$(sha256sum "${archive}" | awk '{ print $1 }')"
else
    archive_sha256="$(shasum -a 256 "${archive}" | awk '{ print $1 }')"
fi
printf '%s  %s\n' "${archive_sha256}" "${package_name}.tar.gz" \
    >"${archive}.sha256"
printf '%s\n' "${archive}"
