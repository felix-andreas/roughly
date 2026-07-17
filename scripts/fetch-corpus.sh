#!/usr/bin/env bash
#
# Populate a gitignored corpus/ directory with real-world R sources used to
# exercise the hand-written parser (see crates/syntax/tests/test_corpus.rs) and
# to stage tree-sitter's own grammar tests (see scripts/convert-tsr-corpus.py).
#
# Idempotent: each subtree is rebuilt from scratch on every run, so re-running
# converges to the same state. Downloaded tarballs are deleted after their R
# files are extracted (disk is limited). The resolved inventory is written to
# scripts/corpus-manifest.txt (committed).
#
# GitHub is not directly reachable here, so the tree-sitter-r corpus is fetched
# through jsDelivr. CRAN is reachable directly.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CORPUS_DIR="$REPO_ROOT/corpus"
MANIFEST="$SCRIPT_DIR/corpus-manifest.txt"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

# --retry covers transient proxy/network hiccups; --fail turns HTTP errors into
# non-zero exits so a 404 is visible rather than silently written to disk.
fetch() {
  curl -sS --fail --location --retry 4 --retry-delay 2 --max-time 600 "$@"
}

# Minimum writable space (KiB) to keep before adding another CRAN package.
MIN_FREE_KB=2000000
avail_kb() { df -Pk "$CORPUS_DIR" | awk 'NR==2 {print $4}'; }

# Ordered CRAN package list. Adjust names here if any 404; substitutions are
# recorded in the manifest by the fetch loop below.
CRAN_PACKAGES=(
  data.table dplyr ggplot2 rlang purrr tibble stringr tidyr shiny jsonlite
  curl R6 cli glue magrittr lubridate readr forcats scales testthat
  knitr withr fs processx callr crayon digest htmltools httpuv mime
  rmarkdown xfun yaml Rcpp stringi vctrs pillar lifecycle generics backports
  checkmate
)

TSR_REF="main"
TSR_FILES=()
R_VERSION=""
R_BASE_LIBS=0
R_BASE_FILES=0
declare -a CRAN_RESULTS=()

echo "==> corpus root: $CORPUS_DIR"

# --------------------------------------------------------------------------
# 1. tree-sitter-r test corpus (via jsDelivr)
# --------------------------------------------------------------------------
echo "==> [1/3] tree-sitter-r corpus (ref: $TSR_REF)"
rm -rf "$CORPUS_DIR/tree-sitter-r"
mkdir -p "$CORPUS_DIR/tree-sitter-r"
listing="$WORK_DIR/tsr-listing.json"
fetch "https://data.jsdelivr.com/v1/packages/gh/r-lib/tree-sitter-r@${TSR_REF}?structure=flat" -o "$listing"
mapfile -t corpus_paths < <(jq -r '.files[].name | select(startswith("/test/corpus/"))' "$listing")
for path in "${corpus_paths[@]}"; do
  base="$(basename "$path")"
  fetch "https://cdn.jsdelivr.net/gh/r-lib/tree-sitter-r@${TSR_REF}${path}" -o "$CORPUS_DIR/tree-sitter-r/$base"
  TSR_FILES+=("$base")
  echo "    fetched $base"
done

# --------------------------------------------------------------------------
# 2. R base-library sources (newest R-4.x tarball)
# --------------------------------------------------------------------------
echo "==> [2/3] R base library sources"
rm -rf "$CORPUS_DIR/r-base"
mkdir -p "$CORPUS_DIR/r-base"
r_index="$WORK_DIR/r-index.html"
fetch "https://cran.r-project.org/src/base/R-4/" -o "$r_index"
R_TARBALL="$(grep -oE 'R-4\.[0-9]+\.[0-9]+\.tar\.gz' "$r_index" | sort -uV | tail -1)"
R_VERSION="${R_TARBALL#R-}"; R_VERSION="${R_VERSION%.tar.gz}"
echo "    newest: $R_TARBALL"
r_tar="$WORK_DIR/$R_TARBALL"
fetch "https://cran.r-project.org/src/base/R-4/$R_TARBALL" -o "$r_tar"
# Extract only src/library/<lib>/R/<file>.R (one directory level, as documented).
members="$WORK_DIR/r-members.txt"
tar tzf "$r_tar" | grep -E '^[^/]+/src/library/[^/]+/R/[^/]+\.R$' > "$members" || true
extract="$WORK_DIR/r-extract"
mkdir -p "$extract"
tar xzf "$r_tar" -C "$extract" -T "$members"
while IFS= read -r member; do
  lib="$(echo "$member" | sed -E 's#^[^/]+/src/library/([^/]+)/R/[^/]+\.R$#\1#')"
  file="$(basename "$member")"
  mkdir -p "$CORPUS_DIR/r-base/$lib"
  cp "$extract/$member" "$CORPUS_DIR/r-base/$lib/$file"
done < "$members"
rm -f "$r_tar"
R_BASE_LIBS="$(find "$CORPUS_DIR/r-base" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
R_BASE_FILES="$(find "$CORPUS_DIR/r-base" -name '*.R' | wc -l | tr -d ' ')"
echo "    extracted $R_BASE_FILES R files across $R_BASE_LIBS libraries"

# --------------------------------------------------------------------------
# 3. CRAN package sources (current versions from PACKAGES)
# --------------------------------------------------------------------------
echo "==> [3/3] CRAN package sources"
rm -rf "$CORPUS_DIR/cran"
mkdir -p "$CORPUS_DIR/cran"
packages_file="$WORK_DIR/PACKAGES"
fetch "https://cran.r-project.org/src/contrib/PACKAGES" -o "$packages_file"

resolve_version() {
  awk -v pkg="$1" '
    BEGIN { RS = ""; FS = "\n" }
    {
      name = ""; ver = ""
      for (i = 1; i <= NF; i++) {
        line = $i; gsub(/\r/, "", line)
        if (line ~ /^Package: /) name = substr(line, 10)
        else if (line ~ /^Version: /) ver = substr(line, 10)
      }
      if (name == pkg) { print ver; exit }
    }' "$packages_file"
}

for pkg in "${CRAN_PACKAGES[@]}"; do
  if [ "$(avail_kb)" -lt "$MIN_FREE_KB" ]; then
    echo "    SKIP $pkg (low disk: $(avail_kb) KiB free)"
    CRAN_RESULTS+=("$pkg	-	skipped-low-disk")
    continue
  fi
  version="$(resolve_version "$pkg" || true)"
  if [ -z "$version" ]; then
    echo "    MISS $pkg (not in PACKAGES)"
    CRAN_RESULTS+=("$pkg	-	not-found")
    continue
  fi
  tarball="${pkg}_${version}.tar.gz"
  pkg_tar="$WORK_DIR/$tarball"
  if ! fetch "https://cran.r-project.org/src/contrib/$tarball" -o "$pkg_tar"; then
    echo "    MISS $pkg ($tarball 404)"
    CRAN_RESULTS+=("$pkg	$version	download-failed")
    continue
  fi
  pkg_members="$WORK_DIR/pkg-members.txt"
  tar tzf "$pkg_tar" | grep -E "^${pkg}/R/[^/]+\.R$" > "$pkg_members" || true
  if [ ! -s "$pkg_members" ]; then
    echo "    note $pkg has no R/*.R files"
    rm -f "$pkg_tar"
    CRAN_RESULTS+=("$pkg	$version	no-R-files")
    continue
  fi
  pkg_extract="$WORK_DIR/pkg-extract"
  rm -rf "$pkg_extract"; mkdir -p "$pkg_extract"
  tar xzf "$pkg_tar" -C "$pkg_extract" -T "$pkg_members"
  mkdir -p "$CORPUS_DIR/cran/$pkg"
  while IFS= read -r member; do
    cp "$pkg_extract/$member" "$CORPUS_DIR/cran/$pkg/$(basename "$member")"
  done < "$pkg_members"
  rm -f "$pkg_tar"
  count="$(find "$CORPUS_DIR/cran/$pkg" -name '*.R' | wc -l | tr -d ' ')"
  echo "    fetched $pkg $version ($count R files)"
  CRAN_RESULTS+=("$pkg	$version	ok")
done

# --------------------------------------------------------------------------
# Manifest
# --------------------------------------------------------------------------
cran_ok="$(printf '%s\n' "${CRAN_RESULTS[@]}" | awk -F'\t' '$3=="ok"' | wc -l | tr -d ' ')"
total_r_files="$(find "$CORPUS_DIR/r-base" "$CORPUS_DIR/cran" -name '*.R' 2>/dev/null | wc -l | tr -d ' ')"

{
  echo "# Roughly parser corpus — pinned inventory"
  echo "# Generated by scripts/fetch-corpus.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)."
  echo "# corpus/ itself is gitignored; re-run scripts/fetch-corpus.sh to repopulate it."
  echo
  echo "[tree-sitter-r]"
  echo "ref = $TSR_REF"
  echo "# jsDelivr exposes no branch commit SHA; nearest published tag is 1.3.0 (matches the workspace tree-sitter-r dep)."
  echo "corpus_files = ${TSR_FILES[*]}"
  echo
  echo "[r-base]"
  echo "version = $R_VERSION"
  echo "source = https://cran.r-project.org/src/base/R-4/R-$R_VERSION.tar.gz"
  echo "libraries = $R_BASE_LIBS"
  echo "r_files = $R_BASE_FILES"
  echo
  echo "[cran]"
  echo "# package<TAB>version<TAB>status  (ok | not-found | download-failed | no-R-files | skipped-low-disk)"
  echo "resolved_ok = $cran_ok / ${#CRAN_PACKAGES[@]}"
  printf '%s\n' "${CRAN_RESULTS[@]}"
  echo
  echo "[totals]"
  echo "parseable_r_files = $total_r_files  (r-base + cran, round-trip + acceptance corpus)"
} > "$MANIFEST"

echo "==> wrote manifest: $MANIFEST"
echo "==> total parseable R files (r-base + cran): $total_r_files"
echo "==> corpus size: $(du -sh "$CORPUS_DIR" | awk '{print $1}')"
