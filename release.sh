#!/usr/bin/env bash
set -euo pipefail

# Script untuk membuat release Linux (tarball & GitHub release) untuk Red Text Editor

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# 1. Ambil versi dari Cargo.toml
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d '"' -f2)
ARCH=$(uname -m)
APP_NAME="red_text_editor"
TAG="v${VERSION}"
RELEASE_NAME="${APP_NAME}-${TAG}-linux-${ARCH}"
DIST_DIR="${REPO_ROOT}/dist"
PKG_DIR="${DIST_DIR}/${RELEASE_NAME}"
ARCHIVE="${DIST_DIR}/${RELEASE_NAME}.tar.gz"

echo "=============================================="
echo " Packaging Release: ${RELEASE_NAME}"
echo "=============================================="

# 2. Build release binary
echo "==> Mengompilasi binary release..."
cargo build --release

# 3. Siapkan folder packaging
echo "==> Menyiapkan folder paket..."
rm -rf "${PKG_DIR}"
mkdir -p "${PKG_DIR}" "${DIST_DIR}"

# Salin binary
cp "${REPO_ROOT}/target/release/${APP_NAME}" "${PKG_DIR}/"

# Salin assets/lang jika ada
if [ -d "${REPO_ROOT}/assets" ]; then
    cp -r "${REPO_ROOT}/assets" "${PKG_DIR}/"
fi

# Salin README
if [ -f "${REPO_ROOT}/README.md" ]; then
    cp "${REPO_ROOT}/README.md" "${PKG_DIR}/"
fi

# Buat file desktop entry installer opsional
cat <<EOF > "${PKG_DIR}/red-text-editor.desktop"
[Desktop Entry]
Type=Application
Name=Red Text Editor
Comment=Editor teks ringan bergaya VS Code
Exec=${APP_NAME}
Terminal=false
Categories=Utility;TextEditor;Development;
EOF

# 4. Buat tar.gz archive
echo "==> Mengompres arsip ${ARCHIVE}..."
tar -C "${DIST_DIR}" -czf "${ARCHIVE}" "${RELEASE_NAME}"
rm -rf "${PKG_DIR}"

# 5. Buat SHA256 checksum
CHECKSUM_FILE="${ARCHIVE}.sha256"
(cd "${DIST_DIR}" && sha256sum "$(basename "${ARCHIVE}")" > "$(basename "${CHECKSUM_FILE}")")

echo ""
echo "Arsip release berhasil dibuat:"
echo "  - Arsip   : ${ARCHIVE}"
echo "  - Checksum: ${CHECKSUM_FILE}"
echo ""

# 6. Pembuatan Tag Git (opsional jika belum ada)
if ! git rev-parse "${TAG}" >/dev/null 2>&1; then
    read -rp "Tag ${TAG} belum ada. Buat git tag lokal dan push ke remote? (y/n): " CREATE_TAG
    if [ "${CREATE_TAG}" = "y" ] || [ "${CREATE_TAG}" = "Y" ]; then
        git tag -a "${TAG}" -m "Release ${TAG}"
        git push origin "${TAG}"
        echo "Tag ${TAG} berhasil dibuat dan di-push ke GitHub."
    fi
else
    echo "Tag ${TAG} sudah ada."
fi

# 7. Upload ke GitHub Release (gh CLI atau instruksi manual)
if command -v gh >/dev/null 2>&1; then
    read -rp "Upload release ke GitHub sekarang menggunakan gh CLI? (y/n): " UPLOAD_GH
    if [ "${UPLOAD_GH}" = "y" ] || [ "${UPLOAD_GH}" = "Y" ]; then
        echo "Mengunggah release ke GitHub..."
        gh release create "${TAG}" "${ARCHIVE}" "${CHECKSUM_FILE}" \
            --title "Release ${TAG}" \
            --notes "Linux release ${TAG} (${ARCH})"
        echo "Release berhasil diunggah ke GitHub!"
    fi
else
    echo "--------------------------------------------------------"
    echo "Tips: 'gh' CLI tidak ditemukan."
    echo "Anda dapat membuat release dan mengunggah file secara manual di:"
    echo "  https://github.com/donairl/red-editor/releases/new?tag=${TAG}"
    echo "File yang perlu diunggah:"
    echo "  - ${ARCHIVE}"
    echo "  - ${CHECKSUM_FILE}"
    echo "--------------------------------------------------------"
fi
