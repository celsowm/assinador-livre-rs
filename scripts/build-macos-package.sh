#!/usr/bin/env bash
set -euo pipefail

target="${TARGET:-x86_64-apple-darwin}"
version="${1:-}"

if [[ -z "${version}" ]]; then
  version="$(sed -nE 's/^version\s*=\s*"([^"]+)"/\1/p' Cargo.toml | head -n1)"
fi

if [[ -z "${version}" ]]; then
  echo "Nao foi possivel determinar versao do Cargo.toml" >&2
  exit 1
fi

bin_path="target/${target}/release/assinador-livre-rs"
pdfium_path="third_party/pdfium/macos-x64/libpdfium.dylib"
icon_path="assets/icone-assinador-livre.png"
out_root="target/packages/macos"
app_name="Assinador Livre RS.app"
app_dir="${out_root}/${app_name}"
contents_dir="${app_dir}/Contents"
macos_dir="${contents_dir}/MacOS"
frameworks_dir="${contents_dir}/Frameworks"
resources_dir="${contents_dir}/Resources"
tarball="${out_root}/assinador-livre-rs-${version}-macos-x64.tar.gz"

if [[ ! -f "${bin_path}" ]]; then
  echo "Binario nao encontrado: ${bin_path}" >&2
  exit 1
fi

if [[ ! -f "${pdfium_path}" ]]; then
  echo "PDFium nao encontrado: ${pdfium_path}" >&2
  exit 1
fi

rm -rf "${out_root}"
mkdir -p "${macos_dir}" "${frameworks_dir}" "${resources_dir}"

cp "${bin_path}" "${macos_dir}/assinador-livre-rs"
cp "${pdfium_path}" "${frameworks_dir}/libpdfium.dylib"
cp "${icon_path}" "${resources_dir}/icone-assinador-livre.png"
chmod +x "${macos_dir}/assinador-livre-rs"

cat > "${contents_dir}/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Assinador Livre RS</string>
  <key>CFBundleDisplayName</key>
  <string>Assinador Livre RS</string>
  <key>CFBundleIdentifier</key>
  <string>com.assinadorlivre.rs</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>CFBundleShortVersionString</key>
  <string>0.0.0</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleExecutable</key>
  <string>assinador-livre-rs</string>
  <key>CFBundleIconFile</key>
  <string>icone-assinador-livre.png</string>
</dict>
</plist>
EOF

version_escaped="${version}"
sed -i.bak "s|<string>0.0.0</string>|<string>${version_escaped}</string>|" "${contents_dir}/Info.plist"
rm -f "${contents_dir}/Info.plist.bak"

tar -C "${out_root}" -czf "${tarball}" "${app_name}"

echo "Pacote macOS gerado:"
echo "- ${tarball}"
