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
sudo apt-get install -y crossbuild-essential-riscv64 rsync qemu-user qemu-user-binfmt

# `dpkg --add-architecture` makes apt request a riscv64 index from every
# configured source, and Ubuntu's own mirrors serve amd64 only — the resulting
# 404s fail `apt-get update` outright. Pin the pre-existing sources to the host
# architecture first; only ports.ubuntu.com carries riscv64.
host_architecture="$(dpkg --print-architecture)"
for source_file in /etc/apt/sources.list.d/*.sources; do
    [[ -e "${source_file}" ]] || continue
    grep -q '^Architectures:' "${source_file}" ||
        sudo sed -i "/^Types:/i Architectures: ${host_architecture}" "${source_file}"
done
if [[ -s /etc/apt/sources.list ]]; then
    sudo sed -i -E "s|^deb ([a-z])|deb [arch=${host_architecture}] \1|" /etc/apt/sources.list
fi

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

# glib's .pc files carry absolute /usr paths and pkg-config prefixes
# PKG_CONFIG_SYSROOT_DIR to them, so the compiler is handed
# <sysroot>/usr/include/glib-2.0 and
# <sysroot>/usr/lib/riscv64-linux-gnu/glib-2.0/include. This sysroot is flat
# (include/, lib/), so bridge the two shapes with symlinks instead of keeping
# a second copy of the tree.
sudo mkdir -p "${sysroot}/usr"
[[ -d "${sysroot}/usr/include" && ! -L "${sysroot}/usr/include" ]] ||
    sudo ln -sfn ../include "${sysroot}/usr/include"
[[ -d "${sysroot}/usr/lib" && ! -L "${sysroot}/usr/lib" ]] ||
    sudo ln -sfn ../lib "${sysroot}/usr/lib"
for triplet_directory in "${sysroot}/lib/riscv64-linux-gnu" "${sysroot}/include/riscv64-linux-gnu"; do
    [[ -d "${triplet_directory}" && ! -L "${triplet_directory}" ]] ||
        sudo ln -sfn . "${triplet_directory}"
done

# Meson looks up the cross ("host machine") pkg-config by prefixed name, and
# neither pkg-config nor crossbuild-essential-riscv64 ships one, so QEMU's
# meson setup fails to find glib. Provide the wrapper, pointed at the sysroot.
sudo tee /usr/bin/riscv64-linux-gnu-pkg-config >/dev/null <<WRAPPER
#!/bin/sh
PKG_CONFIG_LIBDIR="${sysroot}/lib/pkgconfig"
PKG_CONFIG_SYSROOT_DIR="${sysroot}"
export PKG_CONFIG_LIBDIR PKG_CONFIG_SYSROOT_DIR
unset PKG_CONFIG_PATH
exec pkg-config "\$@"
WRAPPER
sudo chmod 0755 /usr/bin/riscv64-linux-gnu-pkg-config

printf 'riscv64 sysroot ready at %s\n' "${sysroot}"
printf 'pkgconfig files: %s\n' "$(find "${sysroot}" -name '*.pc' | wc -l)"
