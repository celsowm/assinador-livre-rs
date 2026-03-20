#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest_path="${repo_root}/third_party/pdfium/manifest.json"

if [[ ! -f "${manifest_path}" ]]; then
  echo "Manifesto PDFium nao encontrado: ${manifest_path}" >&2
  exit 1
fi

python3 - "$manifest_path" <<'PY'
import hashlib
import json
import pathlib
import sys

manifest_path = pathlib.Path(sys.argv[1])
manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
artifacts = manifest.get("artifacts", [])
if not artifacts:
    raise SystemExit("Manifesto PDFium invalido: artifacts ausente.")

base = manifest_path.parent

for artifact in artifacts:
    rel = artifact["file"]
    expected = artifact["sha256"].upper()
    file_path = base / rel
    if not file_path.exists():
        raise SystemExit(f"Arquivo PDFium ausente: {file_path}")

    digest = hashlib.sha256(file_path.read_bytes()).hexdigest().upper()
    if digest != expected:
        raise SystemExit(
            f"Hash PDFium invalido para {rel}. Esperado {expected}, atual {digest}"
        )

print("PDFium hashes validados com sucesso.")
PY
