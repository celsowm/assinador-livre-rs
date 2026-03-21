#!/usr/bin/env bash
set -euo pipefail

NO_BUILD=0
REBUILD=0
NO_LAUNCH=0
KEEP_RUNNING=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build)
      NO_BUILD=1
      shift
      ;;
    --rebuild)
      REBUILD=1
      shift
      ;;
    --no-launch)
      NO_LAUNCH=1
      shift
      ;;
    --keep-running)
      KEEP_RUNNING=1
      shift
      ;;
    *)
      echo "Opcao desconhecida: $1" >&2
      echo "Uso: $0 [--rebuild] [--no-build] [--no-launch] [--keep-running]" >&2
      exit 2
      ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

case "$(uname -s)" in
  Linux)
    TARGET_TRIPLE="x86_64-unknown-linux-gnu"
    PDFIUM_PLATFORM="linux-x64"
    PDFIUM_LIB="libpdfium.so"
    PORTABLE_DIR="$REPO_ROOT/target/linux-user-test"
    ;;
  Darwin)
    TARGET_TRIPLE="x86_64-apple-darwin"
    PDFIUM_PLATFORM="macos-x64"
    PDFIUM_LIB="libpdfium.dylib"
    PORTABLE_DIR="$REPO_ROOT/target/macos-user-test"
    ;;
  *)
    echo "SO nao suportado por este script: $(uname -s)" >&2
    exit 1
    ;;
esac

PORTABLE_EXE="$PORTABLE_DIR/assinador-livre-rs"
PORTABLE_PDFIUM="$PORTABLE_DIR/$PDFIUM_LIB"
RELEASE_EXE="$REPO_ROOT/target/$TARGET_TRIPLE/release/assinador-livre-rs"
SOURCE_PDFIUM="$REPO_ROOT/third_party/pdfium/$PDFIUM_PLATFORM/$PDFIUM_LIB"

if [[ ! -f "$SOURCE_PDFIUM" ]]; then
  echo "PDFium nao encontrado: $SOURCE_PDFIUM" >&2
  exit 1
fi

latest_source_write_time() {
  local newest=0
  local item=""
  while IFS= read -r item; do
    local ts
    if ts="$(stat -c %Y "$item" 2>/dev/null)"; then
      :
    else
      ts="$(stat -f %m "$item")"
    fi
    if (( ts > newest )); then
      newest=$ts
    fi
  done < <(
    {
      [[ -d "$REPO_ROOT/src" ]] && find "$REPO_ROOT/src" -type f
      [[ -d "$REPO_ROOT/ui" ]] && find "$REPO_ROOT/ui" -type f
      [[ -d "$REPO_ROOT/assets" ]] && find "$REPO_ROOT/assets" -type f
      [[ -f "$REPO_ROOT/Cargo.toml" ]] && echo "$REPO_ROOT/Cargo.toml"
      [[ -f "$REPO_ROOT/Cargo.lock" ]] && echo "$REPO_ROOT/Cargo.lock"
      [[ -f "$REPO_ROOT/build.rs" ]] && echo "$REPO_ROOT/build.rs"
    } | awk 'NF'
  )
  echo "$newest"
}

needs_build=0
if (( REBUILD == 1 )); then
  needs_build=1
elif [[ ! -f "$RELEASE_EXE" ]]; then
  needs_build=1
else
  release_ts="$(stat -c %Y "$RELEASE_EXE" 2>/dev/null || stat -f %m "$RELEASE_EXE")"
  source_ts="$(latest_source_write_time)"
  if (( source_ts > release_ts )); then
    needs_build=1
  fi
fi

if (( needs_build == 1 )); then
  if (( NO_BUILD == 1 )); then
    echo "Build necessario, mas --no-build foi informado." >&2
    exit 1
  fi
  if (( REBUILD == 1 )); then
    echo "Rebuild forcado solicitado. Executando build release..."
  else
    echo "Release desatualizado/ausente. Executando build release..."
  fi
  cargo build --release --target "$TARGET_TRIPLE"
fi

if (( KEEP_RUNNING == 0 )); then
  if pgrep -x "assinador-livre-rs" >/dev/null 2>&1; then
    echo "Encerrando instancia em execucao para atualizar o portatil..."
    pkill -x "assinador-livre-rs" || true
  fi
fi

mkdir -p "$PORTABLE_DIR"
cp -f "$RELEASE_EXE" "$PORTABLE_EXE"
cp -f "$SOURCE_PDFIUM" "$PORTABLE_PDFIUM"

echo "Portatil pronto:"
echo " - $PORTABLE_EXE"
echo " - $PORTABLE_PDFIUM"

if (( NO_LAUNCH == 1 )); then
  exit 0
fi

echo "Iniciando app portatil..."
"$PORTABLE_EXE" &
