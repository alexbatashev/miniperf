#!/usr/bin/env bash
# Assembles a riscv64 sysroot at /usr/riscv64-linux-gnu on an amd64 Ubuntu
# runner so DynamoRIO, QEMU, and DuckDB can be cross-compiled there.
#
# Ubuntu's DEB822 sources resolve `apt-get install <pkg>:riscv64` to amd64
# binaries, so this downloads the riscv64 .debs from ports.ubuntu.com and
# unpacks them by hand — the same workaround DynamoRIO's own riscv64 CI uses.
set -euo pipefail

release="${UBUNTU_RELEASE:-noble}"
sysroot=/usr/riscv64-linux-gnu
packages=(
    # DynamoRIO: drsyms and the compression-aware trace tools.
    libunwind8 libunwind-dev
    zlib1g zlib1g-dev
    liblzma5
    libsnappy1v5 libsnappy-dev
    liblz4-1 liblz4-dev
    # QEMU: glib and its transitive development dependencies, plus capstone.
    libglib2.0-0t64 libglib2.0-dev
    libpcre2-8-0 libpcre2-dev
    libffi8 libffi-dev
    libmount1 libmount-dev
    libblkid1 libblkid-dev
    libselinux1 libselinux1-dev
    libsepol2 libsepol-dev
    libcapstone4 libcapstone-dev
)

sudo apt-get update
sudo apt-get install -y crossbuild-essential-riscv64 rsync qemu-user qemu-user-static qemu-user-binfmt

echo "deb [arch=riscv64] http://ports.ubuntu.com/ubuntu-ports ${release} main universe" \
    | sudo tee /etc/apt/sources.list.d/riscv64-ports.list >/dev/null
sudo dpkg --add-architecture riscv64
sudo apt-get update

download_directory="$(mktemp -d)"
extract_directory="$(mktemp -d)"
trap 'rm -rf "${download_directory}" "${extract_directory}"' EXIT

(
    cd "${download_directory}"
    apt-get download "${packages[@]/%/:riscv64}"
    for package in *.deb; do
        dpkg-deb -x "${package}" "${extract_directory}"
    done
)

sudo mkdir -p "${sysroot}/include" "${sysroot}/lib"
for directory in include lib; do
    if [[ -d "${extract_directory}/usr/${directory}/riscv64-linux-gnu" ]]; then
        sudo rsync -a "${extract_directory}/usr/${directory}/riscv64-linux-gnu/" \
            "${sysroot}/${directory}/"
    fi
done
sudo rsync -a "${extract_directory}/usr/include/" "${sysroot}/include/"
if [[ -d "${extract_directory}/lib/riscv64-linux-gnu" ]]; then
    sudo rsync -a "${extract_directory}/lib/riscv64-linux-gnu/" "${sysroot}/lib/"
fi
if [[ -d "${extract_directory}/usr/share/pkgconfig" ]]; then
    sudo mkdir -p "${sysroot}/lib/pkgconfig"
    sudo rsync -a "${extract_directory}/usr/share/pkgconfig/" "${sysroot}/lib/pkgconfig/"
fi

# The unpacked .so symlinks point at absolute /usr/lib/riscv64-linux-gnu paths
# that do not exist on the host; rewrite them relative to the sysroot.
sudo find "${sysroot}/lib" -maxdepth 1 -type l | while read -r link; do
    target="$(readlink "${link}")"
    case "${target}" in
        /usr/lib/riscv64-linux-gnu/* | /lib/riscv64-linux-gnu/*)
            sudo ln -sf "${target##*/}" "${link}"
            ;;
    esac
done

printf 'riscv64 sysroot ready at %s\n' "${sysroot}"
