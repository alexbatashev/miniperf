#!/usr/bin/env bash
set -euo pipefail

qemu_version="${QEMU_VERSION:-11.0.2}"
qemu_sha256="${QEMU_SHA256:-3745f6ea88e2e87fe0dc838b2b1d4e0a770bf48e01a1d5a186842a1fff76ccf5}"
output_directory="${1:-dist}"
build_parent="${QEMU_BUILD_PARENT:-/tmp}"
host_arch="$(uname -m)"
bundle_name="miniperf-qemu-user-${qemu_version}-linux-${host_arch}"
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mkdir -p "${output_directory}" "${build_parent}"
output_directory="$(realpath "${output_directory}")"
build_parent="$(realpath "${build_parent}")"
build_root="$(mktemp -d "${build_parent%/}/miniperf-qemu-build.XXXXXX")"
cleanup() {
    local status=$?
    trap - EXIT
    if [[ "${status}" -ne 0 && "${QEMU_KEEP_FAILED_BUILD:-0}" == 1 ]]; then
        printf 'Preserving failed QEMU build at %s\n' "${build_root}" >&2
    else
        rm -rf "${build_root}"
    fi
    exit "${status}"
}
trap cleanup EXIT

archive="${build_root}/qemu-${qemu_version}.tar.xz"
source_directory="${build_root}/source"
build_directory="${build_root}/build"
install_directory="${build_root}/install"
bundle_directory="${build_root}/${bundle_name}"

if [[ -n "${QEMU_SOURCE_ARCHIVE:-}" ]]; then
    cp "${QEMU_SOURCE_ARCHIVE}" "${archive}"
else
    curl --fail --location --retry 3 \
        "https://download.qemu.org/qemu-${qemu_version}.tar.xz" \
        --output "${archive}"
fi
actual_sha256="$(sha256sum "${archive}" | awk '{print $1}')"
if [[ "${actual_sha256}" != "${qemu_sha256}" ]]; then
    printf 'QEMU source checksum mismatch: expected %s, found %s\n' \
        "${qemu_sha256}" "${actual_sha256}" >&2
    exit 1
fi

mkdir -p "${source_directory}" "${build_directory}" "${install_directory}"
tar --extract --file "${archive}" --directory "${source_directory}" --strip-components=1

(
    cd "${build_directory}"
    "${source_directory}/configure" \
        --python="${QEMU_PYTHON:-/usr/bin/python3}" \
        --cc="${QEMU_CC:-cc}" \
        --cxx="${QEMU_CXX:-c++}" \
        --extra-ldflags='-Wl,-rpath,$ORIGIN/../lib' \
        --disable-download \
        --prefix=/usr \
        --target-list=x86_64-linux-user,riscv32-linux-user,riscv64-linux-user \
        --enable-plugins \
        --enable-capstone \
        --disable-system \
        --disable-bsd-user \
        --disable-docs \
        --disable-tools \
        --disable-guest-agent \
        --without-default-features
    ninja -j "${QEMU_JOBS:-$(nproc)}"
    DESTDIR="${install_directory}" ninja install
)

mkdir -p \
    "${bundle_directory}/bin" \
    "${bundle_directory}/include" \
    "${bundle_directory}/lib" \
    "${bundle_directory}/share/licenses/qemu"
for binary in qemu-x86_64 qemu-riscv32 qemu-riscv64; do
    install -m 0755 "${install_directory}/usr/bin/${binary}" "${bundle_directory}/bin/${binary}"
    strip "${bundle_directory}/bin/${binary}"
done
capstone_soname="$(
    readelf -d "${bundle_directory}/bin/qemu-riscv64" |
        awk '$2 == "(NEEDED)" && $5 ~ /\[libcapstone\.so\./ {
            gsub(/\[|\]/, "", $5)
            print $5
            exit
        }'
)"
if [[ -z "${capstone_soname}" ]]; then
    printf 'Bundled QEMU does not declare a shared Capstone dependency\n' >&2
    exit 1
fi
capstone_library="$(pkg-config --variable=libdir capstone)/${capstone_soname}"
if [[ ! -e "${capstone_library}" ]]; then
    printf 'QEMU requires %s, but pkg-config resolved no such Capstone library at %s\n' \
        "${capstone_soname}" "${capstone_library}" >&2
    exit 1
fi
install -m 0755 \
    "$(realpath "${capstone_library}")" \
    "${bundle_directory}/lib/${capstone_soname}"
install -m 0644 \
    "${install_directory}/usr/include/qemu-plugin.h" \
    "${bundle_directory}/include/qemu-plugin.h"
install -m 0644 \
    "${source_directory}/COPYING" \
    "${bundle_directory}/share/licenses/qemu/COPYING"
install -m 0644 \
    "${source_directory}/COPYING.LIB" \
    "${bundle_directory}/share/licenses/qemu/COPYING.LIB"

"${repository_root}/utils/verify-qemu-roofline-bundle.sh" "${bundle_directory}"

{
    printf 'QEMU_VERSION=%s\n' "${qemu_version}"
    printf 'QEMU_SOURCE_SHA256=%s\n' "${qemu_sha256}"
    printf 'HOST_ARCH=%s\n' "${host_arch}"
    printf 'TARGETS=x86_64-linux-user,riscv32-linux-user,riscv64-linux-user\n'
    printf 'PLUGIN_API=6\n'
    printf 'VERIFICATION=x86_64-sse2,riscv64-rvv\n'
    printf '\nDynamic dependencies (ELF NEEDED):\n'
    (
        cd "${bundle_directory}/bin"
        readelf -d ./qemu-riscv64 | awk '$2 == "(NEEDED)" { gsub(/\[|\]/, "", $5); print $5 }'
    )
} >"${bundle_directory}/MANIFEST.txt"

tarball="${output_directory%/}/${bundle_name}.tar.xz"
tar --create --xz --file "${tarball}" --directory "${build_root}" "${bundle_name}"
(
    cd "${output_directory}"
    bundle_sha256="$(sha256sum "${bundle_name}.tar.xz" | awk '{print $1}')"
    printf '%s  %s\n' "${bundle_sha256}" "${bundle_name}.tar.xz" \
        >"${bundle_name}.tar.xz.sha256"
)
printf '%s\n' "${tarball}"
