#!/usr/bin/env bash
set -euo pipefail

qemu_version="${QEMU_VERSION:-11.0.2}"
qemu_sha256="${QEMU_SHA256:-3745f6ea88e2e87fe0dc838b2b1d4e0a770bf48e01a1d5a186842a1fff76ccf5}"
output_directory="${1:-dist}"
build_parent="${QEMU_BUILD_PARENT:-/tmp}"
host_arch="$(uname -m)"
bundle_name="miniperf-qemu-user-${qemu_version}-linux-${host_arch}"

mkdir -p "${output_directory}" "${build_parent}"
output_directory="$(realpath "${output_directory}")"
build_parent="$(realpath "${build_parent}")"
build_root="$(mktemp -d "${build_parent%/}/miniperf-qemu-build.XXXXXX")"
trap 'rm -rf "${build_root}"' EXIT

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
printf '%s  %s\n' "${qemu_sha256}" "${archive}" | sha256sum --check -

mkdir -p "${source_directory}" "${build_directory}" "${install_directory}"
tar --extract --file "${archive}" --directory "${source_directory}" --strip-components=1

(
    cd "${build_directory}"
    "${source_directory}/configure" \
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
    ninja -j "$(nproc)"
    DESTDIR="${install_directory}" ninja install
)

mkdir -p \
    "${bundle_directory}/bin" \
    "${bundle_directory}/include" \
    "${bundle_directory}/share/licenses/qemu"
for binary in qemu-x86_64 qemu-riscv32 qemu-riscv64; do
    install -m 0755 "${install_directory}/usr/bin/${binary}" "${bundle_directory}/bin/${binary}"
    strip "${bundle_directory}/bin/${binary}"
done
install -m 0644 \
    "${install_directory}/usr/include/qemu-plugin.h" \
    "${bundle_directory}/include/qemu-plugin.h"
install -m 0644 \
    "${source_directory}/COPYING" \
    "${bundle_directory}/share/licenses/qemu/COPYING"
install -m 0644 \
    "${source_directory}/COPYING.LIB" \
    "${bundle_directory}/share/licenses/qemu/COPYING.LIB"

{
    printf 'QEMU_VERSION=%s\n' "${qemu_version}"
    printf 'QEMU_SOURCE_SHA256=%s\n' "${qemu_sha256}"
    printf 'HOST_ARCH=%s\n' "${host_arch}"
    printf 'TARGETS=x86_64-linux-user,riscv32-linux-user,riscv64-linux-user\n'
    printf 'PLUGIN_API=6\n'
    printf '\nDynamic dependencies:\n'
    (
        cd "${bundle_directory}/bin"
        ldd ./qemu-riscv64
    )
} >"${bundle_directory}/MANIFEST.txt"

tarball="${output_directory%/}/${bundle_name}.tar.xz"
tar --create --xz --file "${tarball}" --directory "${build_root}" "${bundle_name}"
(
    cd "${output_directory}"
    sha256sum "${bundle_name}.tar.xz" >"${bundle_name}.tar.xz.sha256"
)
printf '%s\n' "${tarball}"
