#!/usr/bin/env bash
# P7-0 pre-delete freeze: generate or verify the legacy deletion inventory.
#
# The freeze is read-only with respect to the repository. It never runs Node,
# npm, npx, cargo, a test binary or a packager. The only write is an explicit
# `--output PATH` outside the repository.
#
# Usage:
#   scripts/p7-freeze.sh                 # print the freeze JSON to stdout
#   scripts/p7-freeze.sh --check         # compare the worktree to the frozen JSON
#   scripts/p7-freeze.sh --output FILE   # write the JSON outside the repository
#   scripts/p7-freeze.sh --allow-dirty   # generate from an intentionally dirty
#                                        # retarget worktree (never with --check)
#
# `--check` reads docs/evidence/data/legacy-p7-freeze.json and fails when a
# frozen path is missing, a file hash changed, a tree digest changed, an old
# gate moved, or a recorded source anchor disappeared. It is a worktree
# identity check, not a CI gate: the local `backends/typescript/dist` build
# output is part of the frozen identity. A clean checkout must build that
# directory before it can pass the full check.
#
# Identity algorithm (also recorded in the JSON):
#   - regular files only; symlinks and special files are rejected;
#   - root-relative UTF-8 paths with `/` separators;
#   - entries sorted by the raw bytes of the root-relative path under LC_ALL=C;
#   - SHA-256 over `relative_path + NUL + raw_file_bytes + NUL` for every file.
#
# Exit codes:
#   0  generated or checked successfully
#   1  the frozen JSON drifted from the worktree
#   2  usage error, missing tool, missing input, unsafe path, or malformed JSON

set -euo pipefail
umask 022
export LC_ALL=C

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
FROZEN_JSON="$REPO_ROOT/docs/evidence/data/legacy-p7-freeze.json"
DEFAULT_REQUESTED_HEAD='2ce462667b0309046829e8a7c29657fb968ae4b1'

CHECK=0
ALLOW_DIRTY=0
OUTPUT=''

usage() {
  sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
  printf 'p7-freeze: %s\n' "$*" >&2
  exit 2
}

fail() {
  printf 'p7-freeze: %s\n' "$*" >&2
  exit 1
}

while (($# > 0)); do
  case "$1" in
    --check)
      CHECK=1
      ;;
    --output)
      (($# >= 2)) || die '--output requires a path'
      OUTPUT=$2
      shift
      ;;
    --output=*)
      OUTPUT=${1#*=}
      ;;
    --allow-dirty)
      ALLOW_DIRTY=1
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
  shift
done

if ((CHECK == 1)) && [[ -n $OUTPUT ]]; then
  die '--check and --output are mutually exclusive'
fi
if ((CHECK == 1)) && ((ALLOW_DIRTY == 1)); then
  die '--allow-dirty is only for generation; --check already reads the worktree'
fi

for tool in git jq sha256sum realpath awk grep find sort wc cat date; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool is missing: $tool"
done

[[ -e $REPO_ROOT/.git ]] || die "not a git worktree: $REPO_ROOT"

resolve_outside_repo() {
  local path=$1 absolute
  absolute=$(realpath -m -- "$path") || die "cannot resolve --output path: $path"
  case "$absolute" in
    "$REPO_ROOT" | "$REPO_ROOT"/*)
      die "--output must be outside the repository: $absolute"
      ;;
  esac
  printf '%s\n' "$absolute"
}

if [[ -n $OUTPUT ]]; then
  OUTPUT=$(resolve_outside_repo "$OUTPUT")
  [[ -d $(dirname -- "$OUTPUT") ]] || die "output directory does not exist: $(dirname -- "$OUTPUT")"
fi

HEAD=$(git -C "$REPO_ROOT" rev-parse HEAD)
HEAD_SUBJECT=$(git -C "$REPO_ROOT" log -1 --format=%s)
BRANCH=${P7_FREEZE_BRANCH:-$(git -C "$REPO_ROOT" rev-parse --abbrev-ref HEAD)}
REQUESTED_HEAD=${P7_REQUESTED_HEAD:-$DEFAULT_REQUESTED_HEAD}
git -C "$REPO_ROOT" cat-file -e "$REQUESTED_HEAD^{commit}" 2>/dev/null ||
  die "requested baseline commit is missing: $REQUESTED_HEAD"

FROZEN_PATHS=(
  'backends/typescript'
  'crates/wxml-lsp/src/backend_node'
  'crates/wxml-lsp/tests/real_node.rs'
  'crates/wxml-lsp/tests/wave5_acceptance.rs'
  'crates/wxml-lsp/tests/stdio_type_loop.rs'
  'crates/wxml-lsp/tests/performance_bench.rs'
  'crates/wxml-lsp/tests/q2_stability.rs'
  'crates/wxml-lsp/tests/semantic_tools.rs'
  'crates/wxml-lsp/tests/native_parity/native_gates.rs'
  'crates/wxml-lsp/tests/native_parity/ledger.rs'
  'crates/wxml-lsp/tests/native_parity/fixture_hash.rs'
  'crates/wxml-lsp/tests/native_parity/native_engine_real.rs'
  'crates/wxml-lsp/tests/native_parity/symbols_edits.rs'
  'crates/wxml-lsp/tests/native_parity/oracle_manifest.rs'
  'crates/wxml-lsp/tests/native_parity/oracle_manifest_tests.rs'
  'crates/wxml-lsp/tests/native_parity.rs'
  'docs/plans/tsgo-test-ledger.json'
  'docs/plans/tsgo-legacy-oracle-manifest.json'
  'crates/wxml-delivery/src'
)

if ! git -C "$REPO_ROOT" diff --quiet "$REQUESTED_HEAD" HEAD -- "${FROZEN_PATHS[@]}"; then
  die "frozen paths changed between $REQUESTED_HEAD and $HEAD; freeze the reviewed baseline deliberately"
fi
FROZEN_STATUS=$(git -C "$REPO_ROOT" status --porcelain -- "${FROZEN_PATHS[@]}")
if ((CHECK == 0)) && [[ -n $FROZEN_STATUS ]]; then
  ((ALLOW_DIRTY == 1)) ||
    die 'frozen paths have uncommitted changes; commit or revert them before freezing (or pass --allow-dirty for an intentional retarget slice)'
  printf 'p7-freeze: note: generating from a dirty worktree (--allow-dirty); the JSON records frozen_paths_clean=false\n' >&2
fi
if ((CHECK == 1)) && [[ -n $FROZEN_STATUS ]]; then
  printf 'p7-freeze: note: frozen paths have uncommitted changes; checking the worktree as requested\n' >&2
fi

declare -A SHA_CACHE=()
declare -A BYTES_CACHE=()

file_sha() {
  local path=$1 absolute="$REPO_ROOT/$1"
  if [[ -z ${SHA_CACHE[$path]:-} ]]; then
    [[ -f $absolute && ! -L $absolute ]] || die "not a regular file: $path"
    SHA_CACHE[$path]=$(sha256sum -- "$absolute")
    SHA_CACHE[$path]=${SHA_CACHE[$path]%% *}
  fi
  printf '%s' "${SHA_CACHE[$path]}"
}

file_bytes() {
  local path=$1 absolute="$REPO_ROOT/$1"
  if [[ -z ${BYTES_CACHE[$path]:-} ]]; then
    [[ -f $absolute && ! -L $absolute ]] || die "not a regular file: $path"
    BYTES_CACHE[$path]=$(wc -c < "$absolute")
  fi
  printf '%s' "${BYTES_CACHE[$path]}"
}

collect_tracked() {
  local tree=$1
  git -C "$REPO_ROOT" ls-files -z -- "$tree" | LC_ALL=C sort -z
}

collect_untracked() {
  local tree=$1 absolute="$REPO_ROOT/$tree"
  [[ -d $absolute ]] || die "missing local tree: $tree"
  [[ -z $(find "$absolute" -type l -print -quit) ]] ||
    die "refusing symlink in local tree: $tree"
  [[ -z $(find "$absolute" ! -type f ! -type d -print -quit) ]] ||
    die "refusing special file in local tree: $tree"
  find "$absolute" -type f -print0 |
    while IFS= read -r -d '' path; do
      printf '%s\0' "${path#"$REPO_ROOT"/}"
    done |
    LC_ALL=C sort -z
}

# Emits one JSON object for a tree. Paths in `files[]` and the digest input are
# relative to the tree root, not to the repository.
tree_json() {
  local tree=$1 kind=$2 list_files=$3 source=$4
  local -a files=()
  local -a entries=()
  local rel absolute sha bytes total=0 count=0 digest files_json entry

  if [[ $source == tracked ]]; then
    mapfile -d '' files < <(collect_tracked "$tree")
  else
    mapfile -d '' files < <(collect_untracked "$tree")
  fi
  ((${#files[@]} > 0)) || die "empty or missing tree: $tree"

  for rel in "${files[@]}"; do
    [[ $rel == "$tree"/* ]] || die "path escaped $tree: $rel"
    absolute="$REPO_ROOT/$rel"
    [[ -f $absolute && ! -L $absolute ]] || die "not a regular file: $rel"
    sha=$(file_sha "$rel")
    bytes=$(file_bytes "$rel")
    total=$((total + bytes))
    count=$((count + 1))
    if [[ $list_files == 1 ]]; then
      entry=$(jq -cn \
        --arg path "${rel#"$tree"/}" \
        --argjson bytes "$bytes" \
        --arg sha256 "$sha" \
        '{path:$path,bytes:$bytes,sha256:$sha256}')
      entries+=("$entry")
    fi
  done

  digest=$({
    for rel in "${files[@]}"; do
      printf '%s\0' "${rel#"$tree"/}"
      cat -- "$REPO_ROOT/$rel" || exit 1
      printf '\0'
    done
  } | sha256sum | awk '{print $1}')
  [[ $digest =~ ^[0-9a-f]{64}$ ]] || die "tree digest failed for $tree"

  if [[ $list_files == 1 ]]; then
    files_json=$(printf '%s\n' "${entries[@]}" | jq -s '.')
  else
    files_json='[]'
  fi

  jq -cn \
    --arg path "$tree" \
    --arg kind "$kind" \
    --argjson file_count "$count" \
    --argjson total_bytes "$total" \
    --arg tree_sha256 "$digest" \
    --argjson files_listed "$([[ $list_files == 1 ]] && printf true || printf false)" \
    --argjson files "$files_json" \
    '{
      path:$path,
      kind:$kind,
      file_count:$file_count,
      total_bytes:$total_bytes,
      tree_sha256:$tree_sha256,
      files_listed:$files_listed,
      files:$files
    }'
}

# Finds the unique line containing an exact anchor.
anchor_line() {
  local file=$1 anchor=$2 absolute="$REPO_ROOT/$1"
  local count line
  count=$(grep -cF -- "$anchor" "$absolute" || true)
  [[ $count == 1 ]] || die "anchor is missing or ambiguous in $file ($count matches): $anchor"
  line=$(grep -nF -- "$anchor" "$absolute" | cut -d: -f1)
  printf '%s' "$line"
}

# Finds the first anchor line inside a top-level test function, optionally
# starting after a previously located line.
anchor_in_function() {
  local file=$1 start=$2 anchor=$3 after=${4:-$2}
  local absolute="$REPO_ROOT/$file"
  awk -v start="$start" -v after="$after" -v needle="$anchor" '
    NR < after { next }
    NR > start && /^fn / { exit }
    index($0, needle) { print NR; exit }
  ' "$absolute"
}

# Finds the nearest #[ignore] line before a function declaration.
ignore_line_before() {
  local file=$1 function_line=$2
  awk -v end="$function_line" '
    NR < end && /^[[:space:]]*#\[ignore/ { found = NR }
    END { print found + 0 }
  ' "$REPO_ROOT/$file"
}

gate_action() {
  case "$1" in
    G-P5-E2E-0[1-5] | G-P5-E2E-1[5-9] | G-P5-E2E-2[01])
      printf 'delete'
      ;;
    G-P5-E2E-0[6-9] | G-P5-E2E-1[0-4] | G-P5-PERF-01 | G-P5-STABILITY-01)
      printf 'retarget'
      ;;
    *)
      die "unknown gate id: $1"
      ;;
  esac
}

node_dependency_json() {
  local file=$1
  case "$file" in
    crates/wxml-lsp/tests/real_node.rs)
      jq -cn '{
        boundary:"NodeBackend::new in the test process",
        opt_in:"WXML_TEST_NODE=1",
        backend_script:"backends/typescript/dist/server.js",
        backend_dir:"backends/typescript",
        env:["WXML_TEST_NODE","WXML_TYPE_BACKEND","WXML_BACKEND_DIR","WXML_TEST_EXTERNAL_BACKEND"],
        node_minimum:"24",
        source:"#[ignore] reason"
      }'
      ;;
    crates/wxml-lsp/tests/wave5_acceptance.rs)
      jq -cn '{
        boundary:"NodeBackend::new through Runner::open",
        opt_in:"WXML_TEST_NODE=1",
        backend_script:"backends/typescript/dist/server.js",
        backend_dir:"backends/typescript",
        env:["WXML_TEST_NODE","WXML_TYPE_BACKEND","WXML_BACKEND_DIR"],
        node_minimum:"24",
        source:"#[ignore] reason"
      }'
      ;;
    crates/wxml-lsp/tests/stdio_type_loop.rs)
      jq -cn '{
        p7_state:"retargeted-native",
        boundary:"spawns <package>/bin/wxml-lsp --stdio --backend native; the gate keeps the external LSP process boundary and a clean shutdown",
        opt_in:"WXML_TEST_NATIVE=1",
        backend_script:null,
        backend_dir:null,
        env:["WXML_TEST_NATIVE","WXML_REQUIRE_REAL_ENGINE","TSGO_BIN","TSGO_ASSET_ROOT","TSGO_TYPINGS_ROOT"],
        node_minimum:null,
        source:"native_package.rs disposable fixture"
      }'
      ;;
    crates/wxml-lsp/tests/performance_bench.rs)
      jq -cn '{
        boundary:"NodeBackend plus a `node --version` probe and git metadata",
        opt_in:"WXML_TEST_NODE=1",
        backend_script:"backends/typescript/dist/server.js",
        backend_dir:"backends/typescript",
        env:["WXML_TEST_NODE"],
        node_minimum:null,
        source:"test helper"
      }'
      ;;
    crates/wxml-lsp/tests/q2_stability.rs)
      jq -cn '{
        boundary:"NodeBackend plus a process-table scan for dist/server.js",
        opt_in:"WXML_TEST_NODE=1",
        backend_script:"backends/typescript/dist/server.js",
        backend_dir:"backends/typescript",
        env:["WXML_TEST_NODE"],
        node_minimum:null,
        source:"test helper"
      }'
      ;;
    *)
      die "no Node dependency mapping for $file"
      ;;
  esac
}

TREES_JSON=$(
  {
    tree_json 'backends/typescript' 'tracked-repository-tree' 1 tracked
    tree_json 'backends/typescript/src' 'tracked-subtree' 1 tracked
    tree_json 'backends/typescript/test' 'tracked-subtree' 1 tracked
    tree_json 'backends/typescript/dist' 'local-build-output' 1 untracked
    tree_json 'crates/wxml-lsp/src/backend_node' 'tracked-repository-tree' 1 tracked
  } | jq -s 'map({key:.path,value:.}) | from_entries'
)

GATES_JSON=$(
  jq -r '.ignored_rust_tests[] | [.gate_id,.file,.test,.status,(.replacement_test // ""),(.replacement_kind // ""),(.old_reason // "")] | @tsv' \
    "$REPO_ROOT/docs/plans/tsgo-test-ledger.json" |
    while IFS=$'\t' read -r gate_id file test status replacement replacement_kind old_reason; do
      absolute="$REPO_ROOT/$file"
      [[ -f $absolute ]] || die "gate source is missing: $file"
      function_matches=$(grep -nF "fn $test(" "$absolute" || true)
      [[ -n $function_matches ]] || die "gate function is missing: $file::$test"
      [[ $(printf '%s\n' "$function_matches" | wc -l) -eq 1 ]] ||
        die "gate function is not unique: $file::$test"
      function_line=$(printf '%s\n' "$function_matches" | cut -d: -f1)
      ignore_line=$(ignore_line_before "$file" "$function_line")
      ((ignore_line > 0)) || die "gate #[ignore] is missing: $file::$test"
      ((ignore_line < function_line)) || die "gate #[ignore] is after the function: $file::$test"
      fully_qualified="$(basename -- "$file" .rs)::$test"
      action=$(gate_action "$gate_id")
      node_json=$(node_dependency_json "$file")
      jq -cn \
        --arg gate_id "$gate_id" \
        --arg file "$file" \
        --arg test "$test" \
        --arg fully_qualified "$fully_qualified" \
        --argjson line_ignore "$ignore_line" \
        --argjson line_fn "$function_line" \
        --arg status "$status" \
        --arg replacement_test "$replacement" \
        --arg replacement_kind "$replacement_kind" \
        --arg old_reason "$old_reason" \
        --arg p7_action "$action" \
        --argjson node_dependency "$node_json" \
        --arg file_sha256 "$(file_sha "$file")" \
        '{
          gate_id:$gate_id,
          file:$file,
          test:$test,
          fully_qualified:$fully_qualified,
          line_ignore:$line_ignore,
          line_fn:$line_fn,
          status:$status,
          mandatory_green:false,
          replacement_reviewed:false,
          old_test_present:true,
          replacement_test:$replacement_test,
          replacement_kind:$replacement_kind,
          old_reason:$old_reason,
          p7_action:$p7_action,
          node_dependency:$node_dependency,
          file_sha256:$file_sha256
        }'
    done | jq -s 'sort_by(.gate_id)'
)

SILENT_JSON=$(
  jq -r '.silent_skip_gates[] | [.gate_id,.file,.test,.line] | @tsv' \
    "$REPO_ROOT/docs/plans/tsgo-test-ledger.json" |
    while IFS=$'\t' read -r gate_id file test ledger_line; do
      absolute="$REPO_ROOT/$file"
      [[ -f $absolute ]] || die "silent-return source is missing: $file"
      function_line=$(grep -nF "fn $test(" "$absolute" | head -n 1 | cut -d: -f1)
      [[ -n $function_line ]] || die "silent-return function is missing: $file::$test"
      [[ $function_line == "$ledger_line" ]] ||
        die "silent-return ledger line drifted: $file::$test expected $ledger_line got $function_line"
      asset_line=$(anchor_in_function "$file" "$function_line" 'let Some((server, backend_dir)) = real_node_paths() else {')
      [[ -n $asset_line ]] || die "silent-return asset lookup is missing: $file::$test"
      return_line=$(anchor_in_function "$file" "$function_line" 'return;' "$asset_line")
      [[ -n $return_line ]] || die "silent-return statement is missing: $file::$test"
      tsc_line=''
      tsc_skip_line=''
      if [[ $gate_id == 'G-P5-E2E-22' ]]; then
        tsc_line=$(anchor_in_function "$file" "$function_line" 'if tsc.exists() {' "$asset_line")
        tsc_skip_line=$(anchor_in_function "$file" "$function_line" 'tsc not installed; skipped post-edit type check' "$asset_line")
        [[ -n $tsc_line ]] || die "optional tsc branch is missing: $file::$test"
        [[ -n $tsc_skip_line ]] || die "optional tsc skip is missing: $file::$test"
      fi
      jq -cn \
        --arg gate_id "$gate_id" \
        --arg file "$file" \
        --arg test "$test" \
        --argjson line_fn "$function_line" \
        --argjson line_asset_lookup "$asset_line" \
        --argjson line_silent_return "$return_line" \
        --argjson line_tsc_branch "${tsc_line:-null}" \
        --argjson line_tsc_skip "${tsc_skip_line:-null}" \
        --arg file_sha256 "$(file_sha "$file")" \
        '{
          gate_id:$gate_id,
          file:$file,
          test:$test,
          line_fn:$line_fn,
          line_asset_lookup:$line_asset_lookup,
          line_silent_return:$line_silent_return,
          line_tsc_branch:$line_tsc_branch,
          line_tsc_skip:$line_tsc_skip,
          file_sha256:$file_sha256
        }'
    done | jq -s 'sort_by(.gate_id)'
)

LEGACY_ENV_JSON=$(
  cat <<'EOF' |
crates/wxml-lsp/src/backend_node/config.rs|WXML_NODE|if let Some(node) = lookup("WXML_NODE") {|NodeBackendConfig::from_env|Node executable name or absolute path; node backend only
crates/wxml-lsp/src/backend_node/config.rs|WXML_TYPE_BACKEND|if let Some(server) = lookup("WXML_TYPE_BACKEND").map(PathBuf::from) {|NodeBackendConfig::from_env|Absolute server.js path; relative values disable the backend
crates/wxml-lsp/src/backend_node/config.rs|WXML_BACKEND_DIR|if let Some(dir) = lookup("WXML_BACKEND_DIR").map(PathBuf::from) {|NodeBackendConfig::from_env|Absolute backend directory; relative values disable the backend
crates/wxml-lsp/src/backend_node/config.rs|WXML_ANALYZE_TIMEOUT_MS|if let Some(value) = lookup("WXML_ANALYZE_TIMEOUT_MS") {|NodeBackendConfig::from_env|Per-request Node timeout, default 15000
crates/wxml-lsp/src/backend_node/config.rs|WXML_TEMPLATE_CHECK|if let Some(value) = lookup("WXML_TEMPLATE_CHECK") {|NodeBackendConfig::from_env|off, pragma or inherit; node backend only
EOF
  while IFS='|' read -r file name anchor reader note; do
    [[ -n $file && -n $name && -n $anchor && -n $reader && -n $note ]] ||
      die "malformed legacy env row: $name"
    jq -cn \
      --arg name "$name" \
      --arg file "$file" \
      --argjson line "$(anchor_line "$file" "$anchor")" \
      --arg anchor "$anchor" \
      --arg reader "$reader" \
      --arg note "$note" \
      --arg file_sha256 "$(file_sha "$file")" \
      '{name:$name,file:$file,line:$line,anchor:$anchor,reader:$reader,note:$note,file_sha256:$file_sha256}'
  done | jq -s 'sort_by(.name)'
)

GATE_ONLY_ENV_JSON=$(
  cat <<'EOF' |
WXML_TEST_NODE|crates/wxml-lsp/tests/real_node.rs|crates/wxml-lsp/tests/wave5_acceptance.rs|crates/wxml-lsp/tests/performance_bench.rs|crates/wxml-lsp/tests/q2_stability.rs
WXML_TEST_EXTERNAL_BACKEND|crates/wxml-lsp/tests/real_node.rs
EOF
  while IFS='|' read -r name file1 file2 file3 file4 file5; do
    files=()
    for file in "$file1" "$file2" "$file3" "$file4" "$file5"; do
      [[ -n $file ]] && files+=("$file")
    done
    [[ -n $name && ${#files[@]} -gt 0 ]] || die "malformed gate-only env row: $name"
    jq -cn --arg name "$name" --argjson files "$(printf '%s\n' "${files[@]}" | jq -R . | jq -s .)" \
      '{name:$name,kind:"test-only opt-in; deleted with the old Node gates",files:$files}'
  done | jq -s 'sort_by(.name)'
)

V1_DELIVERY_JSON=$(
  cat <<'EOF' |
crates/wxml-delivery/src/bin/wxml-lsp-package.rs|run|fn run() -> Result<(), String> {|delete|CLI dispatch; the no-subcommand/default path still builds schema v1
crates/wxml-delivery/src/bin/wxml-lsp-package.rs|build_package_call|build_package(&options).map_err(|delete|v1 packager call site
crates/wxml-delivery/src/package.rs|build_package|pub fn build_package(options: &BuildOptions) -> Result<BuildReport> {|delete|v1 package entry point
crates/wxml-delivery/src/package.rs|build_manifest|fn build_manifest(options: &BuildOptions, files: &[SourceFile]) -> Result<Manifest> {|delete|v1 schema manifest constructor
crates/wxml-delivery/src/package.rs|update_index|fn update_index(|delete|v1 index.json writer
crates/wxml-delivery/src/install.rs|install_tree|pub fn install_tree(&self, tree: &Path, version: &str) -> Result<InstallOutcome> {|delete|v1 install write path into versions/current
crates/wxml-delivery/src/bin/wxml-lsp-install.rs|install|fn install(options: &Options) -> Result<()> {|delete|schema v1 install branch
crates/wxml-delivery/src/bin/wxml-lsp-install.rs|upgrade|fn upgrade(options: &Options) -> Result<()> {|delete|schema v1 upgrade branch
crates/wxml-delivery/src/bin/wxml-lsp-install.rs|install_and_report|fn install_and_report(|delete|v1 install/verify/switch reporting
crates/wxml-delivery/src/bin/wxml-lsp-install.rs|verify_package|fn verify_package(options: &Options) -> Result<()> {|delete|contains the schema v1 verify-package branch
crates/wxml-delivery/src/acquire.rs|acquire|pub fn acquire(options: &AcquireOptions<'_>) -> Result<AcquiredPackage> {|delete|v1 acquire/cache path
crates/wxml-delivery/src/cache.rs|store_verified|pub fn store_verified(|delete|v1 archive cache writer
crates/wxml-delivery/src/verify.rs|check_backend|pub fn check_backend(root: &Path, manifest: &Manifest) -> Result<()> {|delete|requires bin/wxml-type-backend/server.js and node_modules
crates/wxml-delivery/src/verify.rs|verify_archive|pub fn verify_archive(|delete|v1 archive verification entry point
crates/wxml-delivery/src/manifest.rs|parse_index|pub fn parse_index(bytes: &[u8]) -> Result<Index> {|delete|v1 index parser
crates/wxml-delivery/src/install.rs|verify_installed|pub fn verify_installed(&self, version: &str) -> Result<Manifest> {|keep|read-only verification kept for legacy current
crates/wxml-delivery/src/manifest_v2.rs|read_legacy_manifest|pub fn read_legacy_manifest(root: &Path) -> Result<LegacyManifestProjection> {|keep|legacy read-only projection
crates/wxml-delivery/src/package.rs|build_native_package|pub fn build_native_package(options: &BuildOptionsV2) -> Result<BuildReportV2> {|keep|schema v2 native packager; not a deletion candidate
EOF
  while IFS='|' read -r file symbol anchor action role; do
    [[ -n $file && -n $symbol && -n $anchor && -n $action && -n $role ]] ||
      die "malformed v1 delivery row: $file"
    jq -cn \
      --arg file "$file" \
      --arg symbol "$symbol" \
      --argjson line "$(anchor_line "$file" "$anchor")" \
      --arg anchor "$anchor" \
      --arg action "$action" \
      --arg role "$role" \
      --arg file_sha256 "$(file_sha "$file")" \
      '{file:$file,symbol:$symbol,line:$line,anchor:$anchor,action:$action,role:$role,file_sha256:$file_sha256}'
  done | jq -s 'sort_by(.file,.line)'
)

LEGACY_ORACLE_BINDINGS_JSON=$(
  cat <<'EOF' |
crates/wxml-lsp/tests/native_parity/oracle_manifest.rs|ORACLE_MANIFEST_RELATIVE_PATH|pub(crate) const ORACLE_MANIFEST_RELATIVE_PATH: &str =|manifest path constant|archive-authority
crates/wxml-lsp/tests/native_parity/oracle_manifest.rs|LEGACY_TEST_SOURCE_ROOT|pub(crate) const LEGACY_TEST_SOURCE_ROOT: &str = "backends/typescript/test";|legacy source root|archive-authority
crates/wxml-lsp/tests/native_parity/oracle_manifest.rs|ORACLE_BASELINE_HEAD|pub(crate) const ORACLE_BASELINE_HEAD: &str =|frozen oracle baseline head|provenance
crates/wxml-lsp/tests/native_parity/oracle_manifest.rs|FROZEN_MANIFEST_SHA256|pub(crate) const FROZEN_MANIFEST_SHA256: &str =|manifest integrity pin|integrity
crates/wxml-lsp/tests/native_parity/ledger.rs|verify_legacy_oracle|&workspace.join(oracle_manifest::LEGACY_TEST_SOURCE_ROOT),|ledger passes the legacy root to the oracle verifier|mode-selection
crates/wxml-lsp/tests/native_parity/ledger.rs|live_root|let live_root = workspace.join(oracle_manifest::LEGACY_TEST_SOURCE_ROOT);|live root existence selects Live vs Frozen|mode-selection
crates/wxml-lsp/tests/native_parity/fixture_hash.rs|test_tree|path: "backends/typescript/test",|legacy_archived=true; archive accepted when absent|archived-tree
crates/wxml-lsp/tests/native_parity/fixture_hash.rs|samples_tree|path: "backends/typescript/samples",|legacy_archived=true; archive accepted when absent|archived-tree
crates/wxml-lsp/tests/native_parity/fixture_hash.rs|verify_fixture_trees|let statuses = verify_fixture_trees(&workspace, &ledger_value, &manifest, FROZEN_TREES)|fixture gate takes the oracle manifest|archive-authority
EOF
  while IFS='|' read -r file symbol anchor what kind; do
    [[ -n $file && -n $symbol && -n $anchor && -n $what && -n $kind ]] ||
      die "malformed legacy oracle binding row: $file"
    jq -cn \
      --arg file "$file" \
      --arg symbol "$symbol" \
      --argjson line "$(anchor_line "$file" "$anchor")" \
      --arg anchor "$anchor" \
      --arg what "$what" \
      --arg kind "$kind" \
      --arg file_sha256 "$(file_sha "$file")" \
      '{file:$file,symbol:$symbol,line:$line,anchor:$anchor,what:$what,kind:$kind,file_sha256:$file_sha256}'
  done | jq -s 'sort_by(.file,.line)'
)

ORACLE_MANIFEST_RELATIVE='docs/plans/tsgo-legacy-oracle-manifest.json'
[[ -f $REPO_ROOT/$ORACLE_MANIFEST_RELATIVE && ! -L $REPO_ROOT/$ORACLE_MANIFEST_RELATIVE ]] ||
  die "missing legacy oracle manifest: $ORACLE_MANIFEST_RELATIVE"
ORACLE_MANIFEST_JSON=$(
  jq -c \
    --arg path "$ORACLE_MANIFEST_RELATIVE" \
    --argjson bytes "$(file_bytes "$ORACLE_MANIFEST_RELATIVE")" \
    --arg sha256 "$(file_sha "$ORACLE_MANIFEST_RELATIVE")" \
    '{
      path:$path,
      bytes:$bytes,
      sha256:$sha256,
      schema,
      baseline,
      counts,
      projection_sha256,
      fixture_trees,
      support_file
    }' "$REPO_ROOT/$ORACLE_MANIFEST_RELATIVE"
)


REFERENCE_PATHS=(
  'crates/wxml-lsp/src/main.rs'
  'crates/wxml-lsp/src/lib.rs'
  'crates/wxml-lsp/src/backend_node/config.rs'
  'crates/wxml-lsp/tests/real_node.rs'
  'crates/wxml-lsp/tests/wave5_acceptance.rs'
  'crates/wxml-lsp/tests/stdio_type_loop.rs'
  'crates/wxml-lsp/tests/performance_bench.rs'
  'crates/wxml-lsp/tests/q2_stability.rs'
  'crates/wxml-lsp/tests/semantic_tools.rs'
  'crates/wxml-lsp/tests/native_parity/native_gates.rs'
  'crates/wxml-lsp/tests/native_parity/ledger.rs'
  'crates/wxml-lsp/tests/native_parity/fixture_hash.rs'
  'crates/wxml-lsp/tests/native_parity/oracle_manifest.rs'
  'crates/wxml-lsp/tests/native_parity/oracle_manifest_tests.rs'
  'crates/wxml-lsp/tests/native_parity.rs'
  'crates/wxml-lsp/tests/native_parity/native_engine_real.rs'
  'crates/wxml-lsp/tests/native_parity/symbols_edits.rs'
  'docs/plans/tsgo-legacy-oracle-manifest.json'
  'crates/wxml-delivery/src/package.rs'
  'crates/wxml-delivery/src/install.rs'
  'crates/wxml-delivery/src/acquire.rs'
  'crates/wxml-delivery/src/cache.rs'
  'crates/wxml-delivery/src/verify.rs'
  'crates/wxml-delivery/src/manifest.rs'
  'crates/wxml-delivery/src/manifest_v2.rs'
  'crates/wxml-delivery/src/bin/wxml-lsp-package.rs'
  'crates/wxml-delivery/src/bin/wxml-lsp-install.rs'
)

REFERENCES_JSON=$(
  for file in "${REFERENCE_PATHS[@]}"; do
    [[ -f $REPO_ROOT/$file && ! -L $REPO_ROOT/$file ]] || die "not a regular file: $file"
    jq -cn \
      --arg path "$file" \
      --argjson bytes "$(file_bytes "$file")" \
      --arg sha256 "$(file_sha "$file")" \
      '{path:$path,bytes:$bytes,sha256:$sha256}'
  done | jq -s 'sort_by(.path)'
)

LEDGER_STATE_JSON=$(
  jq -c '{
    green:(.cases | map(select(.status == "green")) | length),
    planned:(.cases | map(select(.status == "planned")) | length),
    reviewed:(.cases | map(select(.status == "reviewed")) | length),
    waived:(.cases | map(select(.status == "waived")) | length),
    retired:(.cases | map(select(.status == "retired-with-decision")) | length),
    total:(.cases | length),
    planned_target:(.closeout.planned_target // 0),
    closeout_policy:(.closeout.policy // "none"),
    ignored_gate_green:(.ignored_rust_tests | map(select(.status == "gate_green")) | length),
    ignored_gate_required:(.ignored_rust_tests | map(select(.status == "required_not_migrated")) | length),
    ignored_gate_total:(.ignored_rust_tests | length),
    silent_skip_gates:(.silent_skip_gates | length),
    rust_silent_skips:(.silent_skips.rust_silent_skips | length)
  }' "$REPO_ROOT/docs/plans/tsgo-test-ledger.json"
)

GENERATED_AT=${P7_FREEZE_NOW:-$(date +%Y-%m-%dT%H:%M:%S%:z)}
if git -C "$REPO_ROOT" rev-parse -q --verify refs/tags/p7-pre-delete >/dev/null; then
  TAG_CREATED=true
else
  TAG_CREATED=false
fi

JSON=$(
  jq -n \
    --arg generated_at "$GENERATED_AT" \
    --arg head "$HEAD" \
    --arg head_subject "$HEAD_SUBJECT" \
    --arg branch "$BRANCH" \
    --arg requested_head "$REQUESTED_HEAD" \
    --argjson frozen_paths_clean "$([[ -z $FROZEN_STATUS ]] && printf true || printf false)" \
    --argjson tag_created "$TAG_CREATED" \
    --argjson trees "$TREES_JSON" \
    --argjson gates "$GATES_JSON" \
    --argjson silent "$SILENT_JSON" \
    --argjson legacy_env "$LEGACY_ENV_JSON" \
    --argjson gate_only_env "$GATE_ONLY_ENV_JSON" \
    --argjson v1 "$V1_DELIVERY_JSON" \
    --argjson legacy_oracle_bindings "$LEGACY_ORACLE_BINDINGS_JSON" \
    --argjson oracle_manifest "$ORACLE_MANIFEST_JSON" \
    --argjson ts_test_present "$([[ -d $REPO_ROOT/backends/typescript/test ]] && printf true || printf false)" \
    --argjson ts_samples_present "$([[ -d $REPO_ROOT/backends/typescript/samples ]] && printf true || printf false)" \
    --argjson references "$REFERENCES_JSON" \
    --argjson ledger_state "$LEDGER_STATE_JSON" \
    '{
      schema:"wxml-tsgo-p7-freeze/1",
      generated_at:$generated_at,
      generated_by:"scripts/p7-freeze.sh",
      algorithm:{
        name:"p7-freeze-tree-v1",
        path_base:"tree root",
        path_format:"root-relative UTF-8 with / separators",
        order:"LC_ALL=C byte sort of the root-relative path",
        entry:"relative_path + NUL + raw_file_bytes + NUL",
        digest:"SHA-256, lowercase hex",
        files:"regular files only; symlinks and special files rejected",
        empty_directories:"not represented",
        locale:"LC_ALL=C",
        line_endings:"raw working-tree bytes; a CRLF checkout changes every hash"
      },
      commit:{
        requested_head:$requested_head,
        head:$head,
        head_subject:$head_subject,
        branch:$branch,
        frozen_paths_clean:$frozen_paths_clean,
        frozen_path_diff_requested_to_head:"empty (verified with git diff --quiet)"
      },
      trees:$trees,
      excluded:[
        {
          path:"backends/typescript/node_modules",
          reason:"gitignored npm install tree; contains symlinks in .bin; not repository identity and not part of --check"
        }
      ],
      old_ignored_gates:{
        source:"docs/plans/tsgo-test-ledger.json#ignored_rust_tests",
        count:($gates | length),
        gates:$gates,
        summary:{
          by_status:($gates | group_by(.status) | map({key:.[0].status,value:length}) | from_entries),
          by_action:($gates | group_by(.p7_action) | map({key:.[0].p7_action,value:length}) | from_entries),
          by_file:($gates | group_by(.file) | map({key:.[0].file,value:length}) | from_entries)
        }
      },
      semantic_tools:{
        silent_returns:$silent,
        note:"Two early returns on a missing dist/server.js plus one optional tsc branch; all still present at this commit."
      },
      legacy_env:{
        runtime:$legacy_env,
        gate_only:$gate_only_env
      },
      v1_delivery:{
        write_paths:($v1 | map(select(.action == "delete"))),
        keep_paths:($v1 | map(select(.action == "keep"))),
        note:"P7-5 deletes only the schema v1 write path; legacy current list/current/verify/env and the read-only projection must remain."
      },
      legacy_ts_oracle:{
        manifest:$oracle_manifest,
        bindings:$legacy_oracle_bindings,
        mode:"archive-authoritative; the live tree is re-hashed only while present",
        note:"P7-2 archive binding is committed at this head. The live backends/typescript tree still exists, but ledger.rs and fixture_hash.rs accept its absence through the frozen manifest."
      },
      reference_files:$references,
      blocking_state:{
        ledger:$ledger_state,
        old_gates_still_ignored:true,
        mandatory_green:false,
        replacement_reviewed:false,
        old_ts_oracle_requires_live_tree:false,
        legacy_oracle_archive_bound:true,
        legacy_ts_tree_present:{
          test:$ts_test_present,
          samples:$ts_samples_present
        },
        v1_write_paths_present:true,
        p7_0:{
          freeze_inventory:true,
          tag_created:$tag_created,
          legacy_node_tests_manifest:false,
          ledger_reads_archive_manifest:true,
          fixture_hash_archive_binding:true,
          native_gates_old_test_removed:false,
          complete:false
        },
        deletion_allowed:false,
        blocked_by:[
          (if ($ledger_state.planned == 0 and $ledger_state.planned_target == 0)
           then "ledger closeout target reached: \($ledger_state.green) green / \($ledger_state.reviewed) reviewed / \($ledger_state.waived) waived / \($ledger_state.retired) retired; reviewed/waived are not green"
           else "ledger is \($ledger_state.green) green / \($ledger_state.planned) planned / \($ledger_state.reviewed) reviewed / \($ledger_state.waived) waived / \($ledger_state.retired) retired; planned target \($ledger_state.planned_target)"
           end),
          "all 23 old gates are still #[ignore]; gate_green is opt-in, not mandatory_green",
          "no replacement_reviewed evidence exists for the 23 gates",
          "native_gates.rs still requires the 23 old #[ignore] tests; old_test_removed is not implemented",
          "the live backends/typescript test/samples trees are still present; P7-4 samples migration is not complete",
          "v1 packager and install write paths are still reachable",
          "the two semantic_tools early returns and the optional tsc branch are still present"
        ]
      },
      scope_notes:[
        "This file freezes identity only; it does not authorize deletion.",
        "backends/typescript is the tracked tree (99 files); dist is the untracked local build output (34 files).",
        "node_modules is an ignored npm install tree and is deliberately outside --check.",
        "Line numbers are worktree line numbers at the recorded head."
      ]
    }'
)

if ((CHECK == 1)); then
  [[ -f $FROZEN_JSON ]] || die "frozen JSON is missing: $FROZEN_JSON"
  jq empty "$FROZEN_JSON" 2>/dev/null || die "frozen JSON is not valid JSON: $FROZEN_JSON"
  expected=$(jq -S 'del(.generated_at, .commit.head, .commit.head_subject, .commit.branch)' "$FROZEN_JSON")
  current=$(printf '%s\n' "$JSON" | jq -S 'del(.generated_at, .commit.head, .commit.head_subject, .commit.branch)')
  if ! diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$current"); then
    fail 'frozen JSON drifted from the worktree; regenerate the evidence deliberately'
  fi
  printf 'p7-freeze: OK (%s; tracked trees and local dist match %s)\n' "$HEAD" "${FROZEN_JSON#"$REPO_ROOT"/}"
  exit 0
fi

if [[ -n $OUTPUT ]]; then
  printf '%s\n' "$JSON" > "$OUTPUT"
  printf 'p7-freeze: wrote %s\n' "$OUTPUT" >&2
else
  printf '%s\n' "$JSON"
fi
