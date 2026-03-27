#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/transfer-codex-sessions.sh \
    --old-host HOST \
    --new-host HOST \
    --old-root PATH \
    --new-root PATH \
    --old-codex-home PATH \
    --new-codex-home PATH \
    [options]

Transfers Codex session state from one machine to another by:
  - selecting rollout files whose saved cwd matches --old-root
  - copying only those rollout files from the old host
  - optionally rewriting saved cwd metadata from --old-root to --new-root
  - merging matching history/session-index entries on the new host

Required options:
  --old-host HOST          Source host to read from.
  --new-host HOST          Destination host to write to.
  --old-root PATH          Source project root used to select sessions.
  --new-root PATH          Destination project root used when rewriting cwd metadata.
  --old-codex-home PATH    Source CODEX_HOME on the old host.
  --new-codex-home PATH    Destination CODEX_HOME on the new host.

Optional behavior:
  --no-rewrite-cwd         Preserve saved cwd metadata from the old host.
  --no-archived            Skip archived sessions.
  --help                   Show this help text.
EOF
}

require_cmd() {
  local command_name=$1
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'Missing required command: %s\n' "$command_name" >&2
    exit 1
  fi
}

resolve_remote_home() {
  local host=$1
  ssh "$host" 'printf "%s" "$HOME"'
}

normalize_remote_path() {
  local raw_path=$1
  local remote_home=$2

  case "$raw_path" in
    "~")
      printf '%s\n' "$remote_home"
      ;;
   "~/"*)
      printf '%s/%s\n' "$remote_home" "${raw_path:2}"
      ;;
    *)
      printf '%s\n' "$raw_path"
      ;;
  esac
}

discover_sessions() {
  local host=$1
  local codex_home=$2
  local project_root=$3
  local include_archived=$4
  local manifest_path=$5

  ssh "$host" bash -s -- "$codex_home" "$project_root" "$include_archived" > "$manifest_path" <<'EOF'
set -euo pipefail

codex_home=$1
project_root=$2
include_archived=$3

cd "$codex_home"

roots=(sessions)
if [[ "$include_archived" == 1 ]]; then
  roots+=(archived_sessions)
fi

for root in "${roots[@]}"; do
  [[ -e "$root" ]] || continue

  while IFS= read -r -d '' file; do
    first_line=$(awk 'NF { print; exit }' "$file")
    [[ -n "$first_line" ]] || continue

    if ! printf '%s\n' "$first_line" | grep -F '"type":"session_meta"' >/dev/null; then
      continue
    fi

    if ! printf '%s\n' "$first_line" | grep -F "\"cwd\":\"$project_root\"" >/dev/null \
      && ! printf '%s\n' "$first_line" | grep -F "\"cwd\":\"$project_root/" >/dev/null; then
      continue
    fi

    thread_id=$(
      printf '%s\n' "$first_line" \
        | grep -Eo '"id":"[0-9a-fA-F-]{36}"' \
        | head -n 1 \
        | cut -d'"' -f4
    )
    [[ -n "$thread_id" ]] || continue

    printf '%s\t%s\n' "$file" "$thread_id"
  done < <(find -L "$root" -type f -name 'rollout-*.jsonl' -print0)
done
EOF
}

rewrite_rollout_cwds() {
  local raw_root=$1
  local file_list=$2
  local old_root=$3
  local new_root=$4

  while IFS= read -r relative_path; do
    [[ -n "$relative_path" ]] || continue

    local source_path="${raw_root}/${relative_path}"
    local temp_path="${source_path}.tmp"

    jq -c --arg old_root "$old_root" --arg new_root "$new_root" '
      def remap($old; $new):
        if . == $old then
          $new
        elif startswith($old + "/") then
          $new + .[($old | length):]
        else
          .
        end;

      if ((.type == "session_meta") or (.type == "turn_context"))
         and (.payload.cwd? | type == "string") then
        .payload.cwd |= remap($old_root; $new_root)
      else
        .
      end
    ' "$source_path" > "$temp_path"

    mv "$temp_path" "$source_path"
  done < "$file_list"
}

maybe_pull_remote_file() {
  local host=$1
  local remote_path=$2
  local local_path=$3

  if ssh "$host" test -f "$remote_path"; then
    rsync -aL "$host:$remote_path" "$local_path"
    return 0
  fi

  return 1
}

filter_jsonl_by_ids() {
  local input_path=$1
  local output_path=$2
  local ids_lookup_path=$3
  local id_expression=$4

  [[ -s "$input_path" ]] || return 0

  jq -c --slurpfile ids "$ids_lookup_path" "select(\$ids[0][($id_expression // \"\")])" "$input_path" \
    | awk '!seen[$0]++' > "$output_path"
}

main() {
  require_cmd ssh
  require_cmd rsync
  require_cmd jq
  require_cmd awk
  require_cmd grep
  require_cmd mktemp

  local old_host=""
  local new_host=""
  local old_root=""
  local new_root=""
  local old_codex_home=""
  local new_codex_home=""
  local rewrite_cwd=1
  local include_archived=1

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --old-host)
        old_host=$2
        shift 2
        ;;
      --new-host)
        new_host=$2
        shift 2
        ;;
      --old-root)
        old_root=$2
        shift 2
        ;;
      --new-root)
        new_root=$2
        shift 2
        ;;
      --old-codex-home)
        old_codex_home=$2
        shift 2
        ;;
      --new-codex-home)
        new_codex_home=$2
        shift 2
        ;;
      --no-rewrite-cwd)
        rewrite_cwd=0
        shift
        ;;
      --no-archived)
        include_archived=0
        shift
        ;;
      --help|-h)
        usage
        exit 0
        ;;
      *)
        printf 'Unknown argument: %s\n\n' "$1" >&2
        usage >&2
        exit 1
        ;;
    esac
  done

  local missing=()
  [[ -n "$old_host" ]] || missing+=("--old-host")
  [[ -n "$new_host" ]] || missing+=("--new-host")
  [[ -n "$old_root" ]] || missing+=("--old-root")
  [[ -n "$new_root" ]] || missing+=("--new-root")
  [[ -n "$old_codex_home" ]] || missing+=("--old-codex-home")
  [[ -n "$new_codex_home" ]] || missing+=("--new-codex-home")

  if [[ ${#missing[@]} -gt 0 ]]; then
    printf 'Missing required arguments: %s\n\n' "${missing[*]}" >&2
    usage >&2
    exit 1
  fi

  local old_home
  old_home=$(resolve_remote_home "$old_host")
  local new_home
  new_home=$(resolve_remote_home "$new_host")

  local old_root_abs
  old_root_abs=$(normalize_remote_path "$old_root" "$old_home")
  local new_root_abs
  new_root_abs=$(normalize_remote_path "$new_root" "$new_home")
  local old_codex_home_abs
  old_codex_home_abs=$(normalize_remote_path "$old_codex_home" "$old_home")
  local new_codex_home_abs
  new_codex_home_abs=$(normalize_remote_path "$new_codex_home" "$new_home")

  local workdir
  workdir=$(mktemp -d "${TMPDIR:-/tmp}/codex-session-transfer.XXXXXX")
  trap "rm -rf -- $(printf '%q' "$workdir")" EXIT

  local manifest_path="${workdir}/manifest.tsv"
  local file_list_path="${workdir}/file-list.txt"
  local ids_path="${workdir}/ids.txt"
  local ids_lookup_path="${workdir}/ids.json"
  local raw_root="${workdir}/raw"
  mkdir -p "$raw_root"

  printf 'Discovering matching sessions on %s...\n' "$old_host"
  discover_sessions "$old_host" "$old_codex_home_abs" "$old_root_abs" "$include_archived" "$manifest_path"

  if [[ ! -s "$manifest_path" ]]; then
    printf 'No sessions found under %s on %s.\n' "$old_root_abs" "$old_host" >&2
    exit 1
  fi

  cut -f1 "$manifest_path" | awk '!seen[$0]++' > "$file_list_path"
  cut -f2 "$manifest_path" | awk '!seen[$0]++' > "$ids_path"
  jq -Rn '[inputs | select(length > 0) | {key: ., value: true}] | from_entries' < "$ids_path" > "$ids_lookup_path"

  local selected_count
  selected_count=$(wc -l < "$file_list_path" | tr -d ' ')
  printf 'Copying %s rollout files from %s...\n' "$selected_count" "$old_host"
  rsync -aL --files-from="$file_list_path" "$old_host:$old_codex_home_abs/" "$raw_root/"

  if [[ "$rewrite_cwd" -eq 1 ]]; then
    printf 'Rewriting saved cwd metadata from %s to %s...\n' "$old_root_abs" "$new_root_abs"
    rewrite_rollout_cwds "$raw_root" "$file_list_path" "$old_root_abs" "$new_root_abs"
  fi

  local old_history_path="${workdir}/history.raw.jsonl"
  local old_session_index_path="${workdir}/session-index.raw.jsonl"
  local history_fragment_path="${workdir}/history.fragment.jsonl"
  local session_index_fragment_path="${workdir}/session-index.fragment.jsonl"

  printf 'Collecting matching history and session-name metadata...\n'
  maybe_pull_remote_file "$old_host" "${old_codex_home_abs}/history.jsonl" "$old_history_path" || true
  maybe_pull_remote_file "$old_host" "${old_codex_home_abs}/session_index.jsonl" "$old_session_index_path" || true

  filter_jsonl_by_ids "$old_history_path" "$history_fragment_path" "$ids_lookup_path" '.session_id // .conversation_id'
  filter_jsonl_by_ids "$old_session_index_path" "$session_index_fragment_path" "$ids_lookup_path" '.id'

  printf 'Ensuring destination Codex home exists on %s...\n' "$new_host"
  ssh "$new_host" mkdir -p "$new_codex_home_abs"

  printf 'Uploading rollout files to %s...\n' "$new_host"
  rsync -a --keep-dirlinks --files-from="$file_list_path" "$raw_root/" "$new_host:$new_codex_home_abs/"

  if [[ -s "$history_fragment_path" || -s "$session_index_fragment_path" ]]; then
    local remote_stage
    remote_stage=$(ssh "$new_host" 'mktemp -d "${TMPDIR:-/tmp}/codex-session-transfer.XXXXXX"')

    if [[ -s "$history_fragment_path" ]]; then
      rsync -a "$history_fragment_path" "$new_host:${remote_stage}/history.fragment.jsonl"
    fi
    if [[ -s "$session_index_fragment_path" ]]; then
      rsync -a "$session_index_fragment_path" "$new_host:${remote_stage}/session-index.fragment.jsonl"
    fi

    printf 'Merging filtered history metadata on %s...\n' "$new_host"
    ssh "$new_host" bash -s -- "$new_codex_home_abs" "$remote_stage" <<'EOF'
set -euo pipefail

codex_home=$1
remote_stage=$2

merge_jsonl() {
  local fragment_path=$1
  local target_path=$2

  [[ -s "$fragment_path" ]] || return 0

  mkdir -p "$(dirname "$target_path")"
  touch "$target_path"
  awk 'NR == FNR { seen[$0] = 1; next } !seen[$0] { print }' "$target_path" "$fragment_path" >> "$target_path"
}

merge_jsonl "${remote_stage}/history.fragment.jsonl" "${codex_home}/history.jsonl"
merge_jsonl "${remote_stage}/session-index.fragment.jsonl" "${codex_home}/session_index.jsonl"
rm -rf "$remote_stage"
EOF
  fi

  local history_count=0
  local session_index_count=0
  if [[ -s "$history_fragment_path" ]]; then
    history_count=$(wc -l < "$history_fragment_path" | tr -d ' ')
  fi
  if [[ -s "$session_index_fragment_path" ]]; then
    session_index_count=$(wc -l < "$session_index_fragment_path" | tr -d ' ')
  fi

  printf '\nTransfer complete.\n'
  printf '  rollouts: %s\n' "$selected_count"
  printf '  history entries: %s\n' "$history_count"
  printf '  session-index entries: %s\n' "$session_index_count"
  if [[ "$rewrite_cwd" -eq 1 ]]; then
    printf '  saved cwd metadata rewritten to: %s\n' "$new_root_abs"
  else
    printf '  saved cwd metadata preserved from: %s\n' "$old_root_abs"
  fi
}

main "$@"
