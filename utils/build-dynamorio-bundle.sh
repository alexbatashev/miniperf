#!/usr/bin/env bash
# Builds a relocatable DynamoRIO bundle for the miniperf roofline backend:
# DynamoRIO (pinned master commit; releases lack the riscv64 port) plus the
# miniperf dr-roofline client linked against roofline-core.
#
# usage: build-dynamorio-bundle.sh [output-directory]
set -euo pipefail

dynamorio_repository="${DYNAMORIO_REPOSITORY:-https://github.com/DynamoRIO/dynamorio.git}"
dynamorio_revision="${DYNAMORIO_REVISION:-1fd3603b213360404f3753fadd0e8e196be1cbdf}"
output_directory="${1:-dist}"
build_parent="${DYNAMORIO_BUILD_PARENT:-/tmp}"
host_arch="$(uname -m)"
bundle_name="miniperf-dynamorio-${dynamorio_revision:0:12}-linux-${host_arch}"
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mkdir -p "${output_directory}" "${build_parent}"
output_directory="$(realpath "${output_directory}")"
build_parent="$(realpath "${build_parent}")"
build_root="$(mktemp -d "${build_parent%/}/miniperf-dynamorio-build.XXXXXX")"
cleanup() {
    local status=$?
    trap - EXIT
    if [[ "${status}" -ne 0 && "${DYNAMORIO_KEEP_FAILED_BUILD:-0}" == 1 ]]; then
        printf 'Preserving failed DynamoRIO build at %s\n' "${build_root}" >&2
    else
        rm -rf "${build_root}"
    fi
    exit "${status}"
}
trap cleanup EXIT

source_directory="${build_root}/source"
build_directory="${build_root}/build"
client_build_directory="${build_root}/client"
bundle_directory="${build_root}/${bundle_name}"

git clone "${dynamorio_repository}" "${source_directory}"
git -C "${source_directory}" checkout --quiet "${dynamorio_revision}"
git -C "${source_directory}" submodule update --init

# ZLIB_ROOT keeps non-system cmake installations (e.g. nix) finding the
# distro zlib that drsyms requires.
cmake="${DYNAMORIO_CMAKE:-cmake}"
"${cmake}" -S "${source_directory}" -B "${build_directory}" \
    -DDISABLE_WARNINGS=ON \
    -DBUILD_TESTS=OFF \
    -DBUILD_SAMPLES=OFF \
    -DBUILD_DOCS=OFF \
    -DZLIB_ROOT=/usr
"${cmake}" --build "${build_directory}" -j "$(nproc)"

cargo build --release -p miniperf-roofline-core \
    --manifest-path "${repository_root}/Cargo.toml"

"${cmake}" -S "${repository_root}/utils/dr-roofline" -B "${client_build_directory}" \
    -DDynamoRIO_DIR="${build_directory}/cmake" \
    -DROOFLINE_CORE_LIB="${repository_root}/target/release/libroofline_core.a"
"${cmake}" --build "${client_build_directory}"

mkdir -p "${bundle_directory}/dynamorio"
for directory in bin64 lib64 ext; do
    cp -a "${build_directory}/${directory}" "${bundle_directory}/dynamorio/"
done
cp "${client_build_directory}/libdr_roofline.so" "${bundle_directory}/"
cp "${source_directory}/License.txt" "${bundle_directory}/DYNAMORIO_LICENSE.txt"
printf 'dynamorio_revision=%s\nhost_arch=%s\n' \
    "${dynamorio_revision}" "${host_arch}" > "${bundle_directory}/MANIFEST"

# Smoke test: the bundled drrun must run the client on a trivial binary.
smoke_output="${build_root}/smoke.counts"
"${bundle_directory}/dynamorio/bin64/drrun" -disable_traces -max_bb_instrs 32 \
    -c "${bundle_directory}/libdr_roofline.so" \
    "output=${smoke_output}" memory-profile=off -- /bin/true
grep -q '^instructions=' "${smoke_output}"

tar --create --file "${output_directory}/${bundle_name}.tar.zst" \
    --zstd --directory "${build_root}" "${bundle_name}"
(cd "${output_directory}" && sha256sum "${bundle_name}.tar.zst" > "${bundle_name}.tar.zst.sha256")
printf 'Bundle written to %s/%s.tar.zst\n' "${output_directory}" "${bundle_name}"
