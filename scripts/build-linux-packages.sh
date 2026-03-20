#!/usr/bin/env bash
set -euo pipefail

target="${TARGET:-x86_64-unknown-linux-gnu}"
version="${1:-}"

if [[ -z "${version}" ]]; then
  version="$(sed -nE 's/^version\s*=\s*"([^"]+)"/\1/p' Cargo.toml | head -n1)"
fi

if [[ -z "${version}" ]]; then
  echo "Nao foi possivel determinar versao do Cargo.toml" >&2
  exit 1
fi

bin_path="target/${target}/release/assinador-livre-rs"
pdfium_path="third_party/pdfium/linux-x64/libpdfium.so"
icon_path="assets/icone-assinador-livre.png"
out_root="target/packages/linux"
rootfs="${out_root}/rootfs"
appdir="${out_root}/AppDir"
appimage_tool="target/tools/appimagetool-x86_64.AppImage"
tarball="${out_root}/assinador-livre-rs-${version}-linux-x64.tar.gz"
appimage="${out_root}/assinador-livre-rs-${version}-linux-x64.AppImage"

if [[ ! -f "${bin_path}" ]]; then
  echo "Binario nao encontrado: ${bin_path}" >&2
  exit 1
fi

if [[ ! -f "${pdfium_path}" ]]; then
  echo "PDFium nao encontrado: ${pdfium_path}" >&2
  exit 1
fi

rm -rf "${out_root}"
mkdir -p "${rootfs}/bin" "${rootfs}/lib" "${rootfs}/assets"
cp "${bin_path}" "${rootfs}/bin/assinador-livre-rs"
cp "${pdfium_path}" "${rootfs}/lib/libpdfium.so"
cp "${icon_path}" "${rootfs}/assets/icone-assinador-livre.png"
chmod +x "${rootfs}/bin/assinador-livre-rs"

mkdir -p "${out_root}"
tar -C "${rootfs}" -czf "${tarball}" .

mkdir -p "${appdir}/usr/bin" "${appdir}/usr/lib" "${appdir}/usr/share/icons/hicolor/256x256/apps"
cp "${rootfs}/bin/assinador-livre-rs" "${appdir}/usr/bin/assinador-livre-rs"
cp "${rootfs}/lib/libpdfium.so" "${appdir}/usr/lib/libpdfium.so"
cp "${icon_path}" "${appdir}/usr/share/icons/hicolor/256x256/apps/assinador-livre-rs.png"
cp "${icon_path}" "${appdir}/assinador-livre-rs.png"

cat > "${appdir}/assinador-livre-rs.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Assinador Livre RS
Exec=assinador-livre-rs
Icon=assinador-livre-rs
Categories=Utility;
Terminal=false
EOF

cat > "${appdir}/AppRun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="${HERE}/usr/lib:${LD_LIBRARY_PATH:-}"
exec "${HERE}/usr/bin/assinador-livre-rs" "$@"
EOF
chmod +x "${appdir}/AppRun"

mkdir -p "$(dirname "${appimage_tool}")"
if [[ ! -f "${appimage_tool}" ]]; then
  curl -fsSL \
    "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage" \
    -o "${appimage_tool}"
  chmod +x "${appimage_tool}"
fi

APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "${appimage_tool}" "${appdir}" "${appimage}"

echo "Pacotes Linux gerados:"
echo "- ${tarball}"
echo "- ${appimage}"
