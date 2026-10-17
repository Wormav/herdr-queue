#!/bin/sh
set -eu
umask 077

PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
VERSION=0.2.0
cd "$PROJECT_DIR"
mkdir -p bin

if [ "${HERDR_QUEUE_BUILD_FROM_SOURCE:-0}" = 1 ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "Compiler les sources nécessite Rust 1.88+ et Cargo." >&2
        exit 1
    fi
    cargo build --release --locked --manifest-path "$PROJECT_DIR/Cargo.toml" --target-dir "$PROJECT_DIR/target"
    cp target/release/herdr-queue bin/herdr-queue.new
    chmod 755 bin/herdr-queue.new
    mv -f bin/herdr-queue.new bin/herdr-queue
    exit 0
fi

if [ -x bin/herdr-queue ] && [ "$(bin/herdr-queue --version)" = "Herdr Queue $VERSION" ]; then
    exit 0
fi

case "$(uname -s)" in
    Darwin) platform=apple-darwin ;;
    Linux) platform=unknown-linux-gnu ;;
    *) echo "Herdr Queue prend en charge macOS et Linux." >&2; exit 1 ;;
esac
case "$(uname -m)" in
    arm64|aarch64) architecture=aarch64 ;;
    x86_64|amd64) architecture=x86_64 ;;
    *) echo "Architecture non prise en charge. Compilez les sources avec Cargo." >&2; exit 1 ;;
esac

archive="herdr-queue-v$VERSION-$architecture-$platform.tar.gz"
release="https://github.com/Wormav/herdr-queue/releases/download/v$VERSION"
install_tmp=$(mktemp -d "$PROJECT_DIR/bin/.install.XXXXXX")
trap 'rm -rf -- "$install_tmp"' EXIT HUP INT TERM

if ! curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error "$release/$archive" --output "$install_tmp/$archive"; then
    echo "Binaire v$VERSION indisponible. Pour compiler : HERDR_QUEUE_BUILD_FROM_SOURCE=1 sh install.sh" >&2
    exit 1
fi
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error "$release/$archive.sha256" --output "$install_tmp/checksum"
expected=$(awk 'NR == 1 { print $1 }' "$install_tmp/checksum")
case "$expected" in ''|*[!0-9a-fA-F]*) echo "Empreinte SHA-256 invalide." >&2; exit 1 ;; esac
if [ "${#expected}" -ne 64 ]; then
    echo "Empreinte SHA-256 invalide." >&2
    exit 1
fi
if command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$install_tmp/$archive" | awk '{ print $1 }')
elif command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$install_tmp/$archive" | awk '{ print $1 }')
else
    echo "Installez shasum ou sha256sum pour vérifier le téléchargement." >&2
    exit 1
fi
if [ "$expected" != "$actual" ]; then
    echo "La vérification SHA-256 a échoué. Aucun binaire installé." >&2
    exit 1
fi
tar -xzf "$install_tmp/$archive" -C "$install_tmp" herdr-queue
if [ ! -f "$install_tmp/herdr-queue" ] || [ -L "$install_tmp/herdr-queue" ]; then
    echo "Le téléchargement ne contient pas le binaire attendu." >&2
    exit 1
fi
chmod 755 "$install_tmp/herdr-queue"
if [ "$("$install_tmp/herdr-queue" --version)" != "Herdr Queue $VERSION" ]; then
    echo "La version téléchargée ne correspond pas à v$VERSION." >&2
    exit 1
fi
mv -f "$install_tmp/herdr-queue" bin/herdr-queue
echo "Herdr Queue $VERSION installé ($architecture-$platform)."
