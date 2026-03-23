#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Install the Codex Agent and Genie APKs and provision device roles.

Usage:
  install-and-provision-agent-genie.sh [options]

Options:
  --serial SERIAL      adb device serial. Defaults to the adb default device.
  --user USER_ID       Android user id for role assignment. Defaults to 0.
  --variant VALUE      APK variant to install: debug or release. Defaults to debug.
  --agent-apk PATH     Agent APK path. Overrides the default path for the selected variant.
  --genie-apk PATH     Genie APK path. Overrides the default path for the selected variant.
  --auth-file PATH     Auth file to seed into the Agent sandbox.
                       Defaults to $HOME/.codex/auth.json when present.
  --skip-auth          Do not copy auth.json into the Agent sandbox.
  --launch-agent       Launch the Agent main activity after provisioning.
  -h, --help           Show this help text.
EOF
}

fail() {
  echo "error: $*" >&2
  exit 1
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
agent_package="com.openai.codex.agent"
genie_package="com.openai.codex.genie"
user_id=0
adb_serial=""
launch_agent=0
skip_auth=0
auth_file="$HOME/.codex/auth.json"
variant="debug"
agent_apk=""
genie_apk=""
agent_apk_overridden=0
genie_apk_overridden=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --serial)
      shift
      [[ $# -gt 0 ]] || fail "--serial requires a value"
      adb_serial="$1"
      ;;
    --user)
      shift
      [[ $# -gt 0 ]] || fail "--user requires a value"
      user_id="$1"
      ;;
    --variant)
      shift
      [[ $# -gt 0 ]] || fail "--variant requires a value"
      variant="$1"
      ;;
    --agent-apk)
      shift
      [[ $# -gt 0 ]] || fail "--agent-apk requires a path"
      agent_apk="$1"
      agent_apk_overridden=1
      ;;
    --genie-apk)
      shift
      [[ $# -gt 0 ]] || fail "--genie-apk requires a path"
      genie_apk="$1"
      genie_apk_overridden=1
      ;;
    --auth-file)
      shift
      [[ $# -gt 0 ]] || fail "--auth-file requires a path"
      auth_file="$1"
      ;;
    --skip-auth)
      skip_auth=1
      ;;
    --launch-agent)
      launch_agent=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

[[ "$variant" == "debug" || "$variant" == "release" ]] || fail "--variant must be debug or release"

if [[ $agent_apk_overridden -eq 0 ]]; then
  agent_apk="$script_dir/app/build/outputs/apk/$variant/app-$variant.apk"
fi
if [[ $genie_apk_overridden -eq 0 ]]; then
  genie_apk="$script_dir/genie/build/outputs/apk/$variant/genie-$variant.apk"
fi
if [[ "$variant" == "release" ]]; then
  if [[ $agent_apk_overridden -eq 0 ]]; then
    agent_apk="$script_dir/app/build/outputs/apk/$variant/app-$variant-unsigned.apk"
  fi
  if [[ $genie_apk_overridden -eq 0 ]]; then
    genie_apk="$script_dir/genie/build/outputs/apk/$variant/genie-$variant-unsigned.apk"
  fi
fi

[[ -f "$agent_apk" ]] || fail "Agent APK not found: $agent_apk"
[[ -f "$genie_apk" ]] || fail "Genie APK not found: $genie_apk"
if [[ "$variant" == "release" && $agent_apk_overridden -eq 0 && "$agent_apk" == *-unsigned.apk ]]; then
  fail "default release Agent APK is unsigned: $agent_apk; sign it first or pass --agent-apk"
fi
if [[ "$variant" == "release" && $genie_apk_overridden -eq 0 && "$genie_apk" == *-unsigned.apk ]]; then
  fail "default release Genie APK is unsigned: $genie_apk; sign it first or pass --genie-apk"
fi

adb_cmd=(adb)
if [[ -n "$adb_serial" ]]; then
  adb_cmd+=(-s "$adb_serial")
fi

"${adb_cmd[@]}" get-state >/dev/null 2>&1 || fail "adb device is not available"

purge_existing_agent_sessions() {
  local sessions_output line session_id parent_session_id initiator_package state top_level_session_id
  local -a all_session_ids=()
  local -a top_level_session_ids=()
  local -a home_session_specs=()

  sessions_output="$(${adb_cmd[@]} shell cmd agent list-sessions "$user_id" 2>/dev/null | tr -d '\r')"
  [[ -n "$sessions_output" ]] || return 0

  while IFS= read -r line; do
    [[ "$line" == AgentSessionInfo\{* ]] || continue
    session_id="$(printf '%s\n' "$line" | sed -n 's/.*sessionId=\([^,}]*\).*/\1/p')"
    parent_session_id="$(printf '%s\n' "$line" | sed -n 's/.*parentSessionId=\([^,}]*\).*/\1/p')"
    initiator_package="$(printf '%s\n' "$line" | sed -n 's/.*initiatorPackage=\([^,}]*\).*/\1/p')"
    state="$(printf '%s\n' "$line" | sed -n 's/.*state=\([0-9][0-9]*\).*/\1/p')"
    [[ -n "$session_id" ]] || continue
    all_session_ids+=("$session_id")
    if [[ "$parent_session_id" == "null" ]]; then
      top_level_session_ids+=("$session_id")
      if [[ "$initiator_package" != "null" ]] && printf '%s\n' "$line" | grep -q 'targetPackage=' && ! printf '%s\n' "$line" | grep -q 'targetPackage=null'; then
        home_session_specs+=("$initiator_package:$session_id:$state")
      fi
    fi
  done <<<"$sessions_output"

  if [[ ${#all_session_ids[@]} -eq 0 ]]; then
    return 0
  fi

  echo "Purging existing framework sessions"
  for session_id in "${all_session_ids[@]}"; do
    "${adb_cmd[@]}" shell cmd agent cancel-session "$session_id" >/dev/null 2>&1 || true
  done

  if (( ${#top_level_session_ids[@]} > 0 )); then
    for top_level_session_id in "${top_level_session_ids[@]}"; do
      local lease_id="codex-install-${top_level_session_id}"
      "${adb_cmd[@]}" shell cmd agent register-ui-lease "$top_level_session_id" "$lease_id" >/dev/null 2>&1 || true
      "${adb_cmd[@]}" shell cmd agent unregister-ui-lease "$top_level_session_id" "$lease_id" >/dev/null 2>&1 || true
    done
  fi

  if (( ${#home_session_specs[@]} > 0 )); then
    for home_spec in "${home_session_specs[@]}"; do
      IFS=':' read -r initiator_package session_id state <<<"$home_spec"
      if [[ "$state" == "4" ]]; then
        "${adb_cmd[@]}" shell cmd agent consume-completed-home-session "$initiator_package" "$session_id" >/dev/null 2>&1 || true
      else
        "${adb_cmd[@]}" shell cmd agent consume-home-session "$initiator_package" "$session_id" >/dev/null 2>&1 || true
      fi
    done
  fi
}

purge_existing_agent_sessions

echo "Stopping existing Agent/Genie processes"
for package_name in \
  "$agent_package" \
  "$genie_package" \
  com.example.agentstub \
  com.example.geniestub \
  com.example.agentstub.standalone \
  com.example.geniestub.standalone; do
  "${adb_cmd[@]}" shell am force-stop "$package_name" >/dev/null 2>&1 || true
done

echo "Installing Agent APK: $agent_apk"
"${adb_cmd[@]}" install -r "$agent_apk"

echo "Installing Genie APK: $genie_apk"
"${adb_cmd[@]}" install -r "$genie_apk"

echo "Assigning AGENT role to $agent_package"
"${adb_cmd[@]}" shell cmd role clear-role-holders --user "$user_id" android.app.role.AGENT 0
"${adb_cmd[@]}" shell cmd role add-role-holder --user "$user_id" android.app.role.AGENT "$agent_package" 0

echo "Assigning GENIE role to $genie_package"
"${adb_cmd[@]}" shell cmd role clear-role-holders --user "$user_id" android.app.role.GENIE 0
"${adb_cmd[@]}" shell cmd role add-role-holder --user "$user_id" android.app.role.GENIE "$genie_package" 0

echo "Granting Agent notification permission"
"${adb_cmd[@]}" shell pm grant "$agent_package" android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true

if [[ $skip_auth -eq 0 && -f "$auth_file" ]]; then
  echo "Seeding Agent auth from $auth_file"
  remote_auth_tmp="/data/local/tmp/codex-agent-auth-${user_id}-$$.json"
  cleanup_remote_auth_tmp() {
    "${adb_cmd[@]}" shell rm -f "$remote_auth_tmp" >/dev/null 2>&1 || true
  }
  trap cleanup_remote_auth_tmp EXIT
  "${adb_cmd[@]}" push "$auth_file" "$remote_auth_tmp" >/dev/null
  "${adb_cmd[@]}" shell run-as "$agent_package" mkdir -p files/codex-home
  "${adb_cmd[@]}" shell \
    "run-as $agent_package sh -c 'cat $remote_auth_tmp > files/codex-home/auth.json && chmod 600 files/codex-home/auth.json'"
  cleanup_remote_auth_tmp
  trap - EXIT
elif [[ $skip_auth -eq 0 ]]; then
  echo "Auth file not found; skipping auth seed: $auth_file"
fi

if [[ $launch_agent -eq 1 ]]; then
  echo "Launching Agent main activity"
  "${adb_cmd[@]}" shell am start -W -n "$agent_package/.MainActivity" >/dev/null
fi

cat <<EOF
Provisioning complete.

Agent package:
  $agent_package

Genie package:
  $genie_package

Role holders:
  AGENT: $("${adb_cmd[@]}" shell cmd role get-role-holders --user "$user_id" android.app.role.AGENT | tr -d '\r')
  GENIE: $("${adb_cmd[@]}" shell cmd role get-role-holders --user "$user_id" android.app.role.GENIE | tr -d '\r')

APK paths:
  Agent: $agent_apk
  Genie: $genie_apk
EOF
