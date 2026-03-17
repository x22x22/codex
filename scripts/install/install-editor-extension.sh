#!/usr/bin/env bash

set -euo pipefail

EXTENSION_ID="openai.chatgpt"
CURSOR_BIN="${CODEX_CURSOR_BIN:-}"
CODE_BIN="${CODEX_CODE_BIN:-}"
CURSOR_DB="${CODEX_CURSOR_DB:-}"
BACKUP_DIR="${CODEX_CURSOR_BACKUP_DIR:-}"
PYTHON_BIN="${CODEX_PYTHON_BIN:-}"

HAD_ERROR=0
UPDATED_CURSOR_STATE=0

step() {
  printf '==> %s\n' "$1"
}

die() {
  printf '%s\n' "$1" >&2
  exit 1
}

default_cursor_db_path() {
  case "$(uname -s)" in
    Darwin)
      printf '%s\n' "${HOME}/Library/Application Support/Cursor/User/globalStorage/state.vscdb"
      ;;
    Linux)
      if [ -n "${XDG_CONFIG_HOME:-}" ]; then
        printf '%s\n' "${XDG_CONFIG_HOME}/Cursor/User/globalStorage/state.vscdb"
      else
        printf '%s\n' "${HOME}/.config/Cursor/User/globalStorage/state.vscdb"
      fi
      ;;
  esac
}

find_editor_bin() {
  local explicit_path="$1"
  local cli_name="$2"
  shift 2

  if [ -n "$explicit_path" ]; then
    [ -x "$explicit_path" ] || die "Editor CLI is not executable: $explicit_path"
    printf '%s\n' "$explicit_path"
    return
  fi

  if command -v "$cli_name" >/dev/null 2>&1; then
    command -v "$cli_name"
    return
  fi

  local candidate_path
  for candidate_path in "$@"; do
    if [ -n "$candidate_path" ] && [ -x "$candidate_path" ]; then
      printf '%s\n' "$candidate_path"
      return
    fi
  done
}

find_python_bin() {
  local explicit_path="$1"
  shift

  if [ -n "$explicit_path" ]; then
    [ -x "$explicit_path" ] || die "Python is not executable: $explicit_path"
    printf '%s\n' "$explicit_path"
    return
  fi

  if command -v python3 >/dev/null 2>&1; then
    command -v python3
    return
  fi

  local candidate_path
  for candidate_path in "$@"; do
    if [ -n "$candidate_path" ] && [ -x "$candidate_path" ]; then
      printf '%s\n' "$candidate_path"
      return
    fi
  done
}

python_can_update_cursor_state() {
  local python_bin="$1"
  "$python_bin" -c 'import json, sqlite3' >/dev/null 2>&1
}

install_extension() {
  local editor_name="$1"
  local editor_bin="$2"

  step "Installing ${EXTENSION_ID} into ${editor_name}"
  "$editor_bin" --install-extension "$EXTENSION_ID" --force
}

process_running() {
  local process_name="$1"
  pgrep -ix "$process_name" >/dev/null 2>&1
}

darwin_app_running() {
  local app_name="$1"
  [ "$(uname -s)" = "Darwin" ] || return 1
  command -v osascript >/dev/null 2>&1 || return 1
  [ "$(osascript -e "application \"${app_name}\" is running" 2>/dev/null || printf 'false')" = "true" ]
}

wait_for_shutdown() {
  local app_name="$1"
  shift
  local process_names=("$@")
  local timeout_seconds=20
  local second=0

  while [ "$second" -lt "$timeout_seconds" ]; do
    local app_stopped=1
    local processes_stopped=1
    local process_name

    if [ -n "$app_name" ] && darwin_app_running "$app_name"; then
      app_stopped=0
    fi

    for process_name in "${process_names[@]}"; do
      if process_running "$process_name"; then
        processes_stopped=0
        break
      fi
    done

    if [ "$app_stopped" -eq 1 ] && [ "$processes_stopped" -eq 1 ]; then
      return 0
    fi

    sleep 1
    second=$((second + 1))
  done

  return 1
}

quit_app_if_running() {
  local editor_name="$1"
  shift

  local app_name=""
  local process_names=()
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --app-name)
        app_name="$2"
        shift 2
        ;;
      *)
        process_names+=("$1")
        shift
        ;;
    esac
  done

  local was_running=0
  local process_name
  if [ -n "$app_name" ] && darwin_app_running "$app_name"; then
    was_running=1
  else
    for process_name in "${process_names[@]}"; do
      if process_running "$process_name"; then
        was_running=1
        break
      fi
    done
  fi

  if [ "$was_running" -eq 0 ]; then
    return 0
  fi

  step "Closing ${editor_name}"

  if [ "$(uname -s)" = "Darwin" ] && [ -n "$app_name" ] && command -v osascript >/dev/null 2>&1; then
    osascript -e "tell application \"${app_name}\" to quit" >/dev/null 2>&1 || true
  else
    printf 'Cannot safely close %s automatically on this platform; please quit it and rerun\n' "$editor_name" >&2
    HAD_ERROR=1
    return 1
  fi

  if ! wait_for_shutdown "$app_name" "${process_names[@]}"; then
    printf 'Failed to close %s safely: %s is still running\n' "$editor_name" "$app_name" >&2
    HAD_ERROR=1
    return 1
  fi

  return 0
}

backup_cursor_db() {
  if [ ! -f "$CURSOR_DB" ]; then
    printf '\n'
    return
  fi

  local backup_root
  if [ -n "$BACKUP_DIR" ]; then
    backup_root="$BACKUP_DIR"
  else
    backup_root="$(dirname "$CURSOR_DB")"
  fi

  mkdir -p "$backup_root"
  find "$backup_root" -maxdepth 1 -type f -name 'state.vscdb.backup.*' -delete >/dev/null 2>&1 || true
  local backup_path="${backup_root}/state.vscdb.backup.$(date +%Y%m%d%H%M%S)"
  cp "$CURSOR_DB" "$backup_path"
  printf '%s\n' "$backup_path"
}

update_cursor_state() {
  local backup_path="$1"

  step "Updating Cursor state in ${CURSOR_DB}"
  if ! "$PYTHON_BIN" - "$CURSOR_DB" <<'PY'
import json
import sqlite3
import sys
import uuid
from pathlib import Path

db_path = Path(sys.argv[1])
db_path.parent.mkdir(parents=True, exist_ok=True)

PIN_KEY = "sidebar2.sidebarData.memoized.v1"
DEFAULT_HIDDEN_KEY = "workbench.view.extension.codexViewContainer.state.hidden"
APP_KEY = "src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser"
VIEWS_CUSTOMIZATIONS_KEY = "views.customizations"
AUXILIARYBAR_PINNED_PANELS_KEY = "workbench.auxiliarybar.pinnedPanels"
AUXILIARYBAR_PLACEHOLDER_PANELS_KEY = "workbench.auxiliarybar.placeholderPanels"

CONTAINER_ID = "workbench.view.extension.codexViewContainer"
VIEW_ID = "chatgpt.sidebarView"
EXTENSION_ID = "openai.chatgpt"
AUXILIARYBAR_PREFIX = "workbench.views.service.auxiliarybar."


def read_json(cur, key, default):
    row = cur.execute("SELECT value FROM ItemTable WHERE key = ?", (key,)).fetchone()
    if row is None or row[0] in (None, ""):
        return default
    return json.loads(row[0])


def write_json(cur, key, value):
    payload = json.dumps(value, separators=(",", ":"))
    cur.execute(
        "INSERT OR REPLACE INTO ItemTable(key, value) VALUES(?, ?)",
        (key, payload),
    )


def delete_key(cur, key):
    cur.execute("DELETE FROM ItemTable WHERE key = ?", (key,))


def write_json_to_cursor_disk_kv(cur, key, value):
    payload = json.dumps(value, separators=(",", ":"))
    cur.execute(
        "INSERT OR REPLACE INTO cursorDiskKV(key, value) VALUES(?, ?)",
        (key, payload),
    )


def candidate_icon_path():
    extensions_dir = Path.home() / ".cursor" / "extensions"
    if not extensions_dir.exists():
        return None

    matches = sorted(
        extensions_dir.glob(f"{EXTENSION_ID}-*/resources/blossom-white.svg"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    if not matches:
        return None
    return str(matches[0])


def is_auxiliarybar_id(value):
    return bool(value) and value.startswith(AUXILIARYBAR_PREFIX)


def generated_auxiliarybar_id():
    stable_uuid = uuid.uuid5(uuid.NAMESPACE_DNS, EXTENSION_ID)
    return f"{AUXILIARYBAR_PREFIX}{stable_uuid}"


def panel_matches_codex(panel):
    panel_id = panel.get("id")
    if not is_auxiliarybar_id(panel_id):
        return False

    if panel.get("name") == "Codex":
        return True

    icon_url = panel.get("iconUrl") or {}
    icon_path = icon_url.get("path", "")
    if f"/{EXTENSION_ID}-" in icon_path:
        return True

    for view in panel.get("views", []):
        if view.get("when") == "chatgpt.doesNotSupportSecondarySidebar":
            return True

    return False


conn = sqlite3.connect(str(db_path))
cur = conn.cursor()
cur.execute(
    "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)"
)
cur.execute(
    "CREATE TABLE IF NOT EXISTS cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)"
)

sidebar_data = read_json(
    cur,
    PIN_KEY,
    {"pinnedViewContainerIDs": [], "viewContainerOrders": {}},
)
pinned_ids = sidebar_data.setdefault("pinnedViewContainerIDs", [])
sidebar_data["pinnedViewContainerIDs"] = [
    item for item in pinned_ids if item != CONTAINER_ID
]
write_json(cur, PIN_KEY, sidebar_data)

default_hidden_data = read_json(cur, DEFAULT_HIDDEN_KEY, [])
updated = False
for item in default_hidden_data:
    if item.get("id") == VIEW_ID:
        item["isHidden"] = False
        updated = True
        break
if not updated:
    default_hidden_data.append({"id": VIEW_ID, "isHidden": False})
write_json(cur, DEFAULT_HIDDEN_KEY, default_hidden_data)

placeholder_panels = read_json(cur, AUXILIARYBAR_PLACEHOLDER_PANELS_KEY, [])
pinned_panels = read_json(cur, AUXILIARYBAR_PINNED_PANELS_KEY, [])
views_customizations = read_json(
    cur,
    VIEWS_CUSTOMIZATIONS_KEY,
    {
        "viewContainerLocations": {},
        "viewLocations": {},
        "viewContainerBadgeEnablementStates": {},
    },
)
view_locations = views_customizations.setdefault("viewLocations", {})
view_container_locations = views_customizations.setdefault(
    "viewContainerLocations", {}
)
existing_container_id = view_locations.get(VIEW_ID)

codex_container_ids = {
    panel.get("id")
    for panel in placeholder_panels
    if panel_matches_codex(panel)
}
if is_auxiliarybar_id(existing_container_id):
    codex_container_ids.add(existing_container_id)

target_container_id = None
if is_auxiliarybar_id(existing_container_id):
    target_container_id = existing_container_id
else:
    for panel in placeholder_panels:
        if panel_matches_codex(panel):
            target_container_id = panel.get("id")
            break

if not target_container_id:
    target_container_id = generated_auxiliarybar_id()
    codex_container_ids.add(target_container_id)

view_locations[VIEW_ID] = target_container_id
view_container_locations[target_container_id] = 2

for container_id in list(codex_container_ids):
    if container_id != target_container_id and container_id not in view_locations.values():
        view_container_locations.pop(container_id, None)

write_json(cur, VIEWS_CUSTOMIZATIONS_KEY, views_customizations)

icon_path = candidate_icon_path()

target_placeholder_panel = None
filtered_placeholder_panels = []
for panel in placeholder_panels:
    panel_id = panel.get("id")
    if panel_id == target_container_id:
        target_placeholder_panel = panel
        continue
    if panel_id in codex_container_ids:
        continue
    filtered_placeholder_panels.append(panel)

if target_placeholder_panel is None:
    target_placeholder_panel = {
        "id": target_container_id,
        "name": "Codex",
        "isBuiltin": True,
        "views": [{"when": "chatgpt.doesNotSupportSecondarySidebar"}],
    }

target_placeholder_panel["id"] = target_container_id
target_placeholder_panel["name"] = "Codex"
target_placeholder_panel["isBuiltin"] = True
target_placeholder_panel["views"] = [{"when": "chatgpt.doesNotSupportSecondarySidebar"}]
if icon_path is not None:
    target_placeholder_panel["iconUrl"] = {
        "$mid": 1,
        "path": icon_path,
        "scheme": "file",
    }

write_json(
    cur,
    AUXILIARYBAR_PLACEHOLDER_PANELS_KEY,
    [target_placeholder_panel, *filtered_placeholder_panels],
)

target_pinned_panel = None
filtered_pinned_panels = []
for panel in pinned_panels:
    panel_id = panel.get("id")
    if panel_id == target_container_id:
        target_pinned_panel = panel
        continue
    if panel_id in codex_container_ids:
        continue
    filtered_pinned_panels.append(panel)

if target_pinned_panel is None:
    target_pinned_panel = {"id": target_container_id}

target_pinned_panel["id"] = target_container_id
target_pinned_panel["pinned"] = True
target_pinned_panel["visible"] = False

write_json(
    cur,
    AUXILIARYBAR_PINNED_PANELS_KEY,
    [target_pinned_panel, *filtered_pinned_panels],
)

write_json(
    cur,
    f"{target_container_id}.state.hidden",
    [{"id": VIEW_ID, "isHidden": False}],
)

for container_id in codex_container_ids:
    if container_id != target_container_id:
        delete_key(cur, f"{container_id}.state.hidden")

app_data = read_json(cur, APP_KEY, {})
ai_settings = app_data.setdefault("aiSettings", {})
model_config = ai_settings.setdefault("modelConfig", {})
composer = model_config.setdefault("composer", {})
composer["modelName"] = "gpt-5.4-medium"
composer["maxMode"] = False
write_json(cur, APP_KEY, app_data)

composer_rows = cur.execute(
    "SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'"
).fetchall()
for key, raw_value in composer_rows:
    if raw_value in (None, ""):
        continue

    try:
        composer_data = json.loads(raw_value)
    except json.JSONDecodeError:
        continue

    if not (
        composer_data.get("isAgentic") is True
        or composer_data.get("unifiedMode") == "agent"
    ):
        continue

    row_model_config = composer_data.setdefault("modelConfig", {})
    row_model_config["modelName"] = "gpt-5.4-medium"
    row_model_config["maxMode"] = False
    write_json_to_cursor_disk_kv(cur, key, composer_data)

conn.commit()
conn.close()
PY
  then
    return 1
  fi

  if [ -n "$backup_path" ]; then
    step "Cursor state backup saved to ${backup_path}"
  fi
  UPDATED_CURSOR_STATE=1
}

apply_cursor_state_changes() {
  if [ -z "$CURSOR_DB" ]; then
    step "Skipping Cursor state changes: unsupported OS for automatic Cursor DB updates"
    return 0
  fi

  mkdir -p "$(dirname "$CURSOR_DB")"

  local backup_path=""
  backup_path="$(backup_cursor_db)"

  if update_cursor_state "$backup_path"; then
    return 0
  fi

  if [ -n "$backup_path" ] && [ -f "$backup_path" ]; then
    printf 'Failed to update Cursor state; restoring backup from %s\n' "$backup_path" >&2
    cp "$backup_path" "$CURSOR_DB" >/dev/null 2>&1 || true
  else
    printf 'Failed to update Cursor state; removing incomplete database at %s\n' "$CURSOR_DB" >&2
    rm -f "$CURSOR_DB" >/dev/null 2>&1 || true
  fi

  HAD_ERROR=1
  return 1
}

if [ "$#" -ne 0 ]; then
  die "This script takes no arguments."
fi

if [ -z "$CURSOR_DB" ]; then
  CURSOR_DB="$(default_cursor_db_path || true)"
fi

resolved_cursor_bin="$(find_editor_bin \
  "$CURSOR_BIN" \
  "cursor" \
  "/Applications/Cursor.app/Contents/Resources/app/bin/cursor" \
  "/opt/Cursor/resources/app/bin/cursor" \
  "/opt/cursor/resources/app/bin/cursor" \
  "/usr/bin/cursor" \
  || true)"
resolved_code_bin="$(find_editor_bin \
  "$CODE_BIN" \
  "code" \
  "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code" \
  "/usr/bin/code" \
  "/snap/bin/code" \
  "/usr/share/code/bin/code" \
  "/opt/visual-studio-code/bin/code" \
  || true)"
resolved_python_bin="$(find_python_bin \
  "$PYTHON_BIN" \
  "/usr/bin/python3" \
  "/opt/homebrew/bin/python3" \
  "/usr/local/bin/python3" \
  || true)"

install_cursor=0
install_vscode=0
cursor_ready=0
vscode_ready=0

if [ -n "$resolved_cursor_bin" ]; then
  install_cursor=1
else
  step "Skipping Cursor: editor is not installed"
fi

if [ -n "$resolved_code_bin" ]; then
  install_vscode=1
else
  step "Skipping VS Code: editor is not installed"
fi

if [ "$install_cursor" -eq 0 ] && [ "$install_vscode" -eq 0 ]; then
  step "No supported editors were detected; nothing to do"
  exit 0
fi

if [ "$install_cursor" -eq 1 ]; then
  if [ -n "$CURSOR_DB" ] && [ -z "$resolved_python_bin" ]; then
    printf 'Skipping Cursor: python3 is required to update Cursor state safely\n' >&2
    HAD_ERROR=1
    install_cursor=0
  elif [ -n "$CURSOR_DB" ] && ! python_can_update_cursor_state "$resolved_python_bin"; then
    printf 'Skipping Cursor: python3 cannot import the modules required to update Cursor state safely\n' >&2
    HAD_ERROR=1
    install_cursor=0
  else
    PYTHON_BIN="$resolved_python_bin"
  fi
fi

if [ "$install_cursor" -eq 1 ]; then
  if quit_app_if_running "Cursor" --app-name "Cursor" Cursor cursor; then
    cursor_ready=1
  else
    step "Skipping Cursor: failed to close the running app safely"
  fi
fi

if [ "$install_vscode" -eq 1 ]; then
  if quit_app_if_running "VS Code" --app-name "Visual Studio Code" "Visual Studio Code" Code code; then
    vscode_ready=1
  else
    step "Skipping VS Code: failed to close the running app safely"
  fi
fi

cursor_installed=0
if [ "$install_cursor" -eq 1 ] && [ "$cursor_ready" -eq 1 ]; then
  if install_extension "Cursor" "$resolved_cursor_bin"; then
    cursor_installed=1
  else
    printf 'Failed to install %s into Cursor\n' "$EXTENSION_ID" >&2
    HAD_ERROR=1
  fi
fi

if [ "$install_vscode" -eq 1 ] && [ "$vscode_ready" -eq 1 ]; then
  if ! install_extension "VS Code" "$resolved_code_bin"; then
    printf 'Failed to install %s into VS Code\n' "$EXTENSION_ID" >&2
    HAD_ERROR=1
  fi
fi

if [ "$cursor_installed" -eq 1 ]; then
  apply_cursor_state_changes || true
fi

if [ "$UPDATED_CURSOR_STATE" -eq 1 ]; then
  step "Cursor state changes applied"
fi

if [ "$HAD_ERROR" -eq 1 ]; then
  exit 1
fi

step "Done"
