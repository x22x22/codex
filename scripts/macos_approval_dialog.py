#!/usr/bin/env python3
"""Show a macOS approval dialog via osascript/JXA with JSON input and output.

The helper wraps the working NSAlert + accessory-view pattern we prototyped:
- frontmost activation
- optional thread / reason / permission-rule labels
- optional monospaced code box
- custom buttons with single-key equivalents
- JSON result on stdout

Input is read from stdin by default, or from --input. Example:

{
  "kind": "exec",
  "thread": "Robie [explorer]",
  "reason": "run the targeted test suite before finalizing changes.",
  "permission_rule": "write `/tmp`, `/Users/ebrevdo/code/codex`",
  "code": "python -m pytest tests/test_example.py\\n--maxfail=1\\n-q"
}
"""

from __future__ import annotations

import argparse
import json
import os
import select
import shlex
import subprocess
import sys
import tempfile
import textwrap
from datetime import datetime
from pathlib import Path
from typing import Any


DEFAULT_WIDTH = 620
DEFAULT_CODE_HEIGHT = 180
BUTTON_WRAP_WIDTH = 72
TITLE_WRAP_WIDTH = 52
BODY_WRAP_WIDTH = 72
BUTTON_VERTICAL_GAP = 4
DEBUG_ENV = "CODEX_APPROVAL_DIALOG_DEBUG"
DEBUG_DIR_ENV = "CODEX_APPROVAL_DIALOG_DEBUG_DIR"
SIMPLE_DIALOG_ENV = "CODEX_APPROVAL_DIALOG_SIMPLE"
OSASCRIPT_TIMEOUT_ENV = "CODEX_APPROVAL_DIALOG_OSASCRIPT_TIMEOUT_MS"
REQUESTER_PID_ENV = "CODEX_APPROVAL_REQUESTER_PID"
STDIN_TIMEOUT_ENV = "CODEX_APPROVAL_DIALOG_STDIN_TIMEOUT_MS"
DEFAULT_STDIN_TIMEOUT_MS = 100


def _render_command(command: list[str]) -> str:
    try:
        return shlex.join(command)
    except Exception:
        return " ".join(command)


def _format_permission_rule(additional_permissions: dict[str, Any] | None) -> str | None:
    if not additional_permissions:
        return None
    parts: list[str] = []
    file_system = additional_permissions.get("file_system")
    if isinstance(file_system, dict):
        read = file_system.get("read")
        if isinstance(read, list) and read:
            parts.append("read " + ", ".join(f"`{item}`" for item in read))
        write = file_system.get("write")
        if isinstance(write, list) and write:
            parts.append("write " + ", ".join(f"`{item}`" for item in write))
    return "; ".join(parts) if parts else None


def _decision_id(decision: Any) -> str:
    if isinstance(decision, str):
        return decision
    if isinstance(decision, dict) and len(decision) == 1:
        return next(iter(decision))
    raise ValueError(f"unsupported review decision shape: {decision!r}")


def _execpolicy_command_prefix(amendment: Any) -> list[str]:
    if isinstance(amendment, list) and all(isinstance(item, str) for item in amendment):
        return amendment
    if isinstance(amendment, dict):
        command = amendment.get("command")
        if isinstance(command, list) and all(isinstance(item, str) for item in command):
            return command
    raise ValueError(f"unsupported execpolicy amendment shape: {amendment!r}")


def _option_for_exec_decision(
    decision: Any,
    *,
    network_approval_context: dict[str, Any] | None,
    additional_permissions: dict[str, Any] | None,
) -> dict[str, Any]:
    decision_id = _decision_id(decision)
    option: dict[str, Any]
    if decision_id == "approved":
        option = {
            "id": "approved",
            "label": "Yes, just this once" if network_approval_context else "Yes, proceed",
            "key": "y",
            "default": True,
        }
    elif decision_id == "approved_execpolicy_amendment":
        amendment = decision["approved_execpolicy_amendment"]
        prefix = _render_command(
            _execpolicy_command_prefix(amendment["proposed_execpolicy_amendment"])
        )
        option = {
            "id": "approved_execpolicy_amendment",
            "label": f"Yes, and don't ask again for commands that start with `{prefix}`",
            "key": "p",
        }
    elif decision_id == "approved_for_session":
        if network_approval_context:
            label = "Yes, and allow this host for this conversation"
        elif additional_permissions:
            label = "Yes, and allow these permissions for this session"
        else:
            label = "Yes, and don't ask again for this command in this session"
        option = {"id": "approved_for_session", "label": label, "key": "a"}
    elif decision_id == "network_policy_amendment":
        amendment = decision["network_policy_amendment"]
        action = amendment["action"]
        if action == "allow":
            option = {
                "id": "network_policy_amendment_allow",
                "label": "Yes, and allow this host in the future",
                "key": "p",
            }
        elif action == "deny":
            option = {
                "id": "network_policy_amendment_deny",
                "label": "No, and block this host in the future",
                "key": "d",
            }
        else:
            raise ValueError(f"unsupported network policy action: {action!r}")
    elif decision_id == "denied":
        option = {
            "id": "denied",
            "label": "No, continue without running it",
            "key": "d",
        }
    elif decision_id == "abort":
        option = {
            "id": "abort",
            "label": "No, and tell Codex what to do differently",
            "key": "n",
            "cancel": True,
        }
    else:
        raise ValueError(f"unsupported exec decision: {decision!r}")

    option["decision"] = decision
    return option


def _default_exec_decisions(raw: dict[str, Any]) -> list[Any]:
    if raw.get("available_decisions") is not None:
        return raw["available_decisions"]
    network_approval_context = raw.get("network_approval_context")
    if network_approval_context is not None:
        decisions: list[Any] = ["approved", "approved_for_session"]
        amendments = raw.get("proposed_network_policy_amendments") or []
        allow_amendment = next(
            (item for item in amendments if isinstance(item, dict) and item.get("action") == "allow"),
            None,
        )
        if allow_amendment is not None:
            decisions.append({"network_policy_amendment": allow_amendment})
        decisions.append("abort")
        return decisions
    if raw.get("additional_permissions") is not None:
        return ["approved", "abort"]
    decisions = ["approved"]
    amendment = raw.get("proposed_execpolicy_amendment")
    if amendment is not None:
        decisions.append({"approved_execpolicy_amendment": {"proposed_execpolicy_amendment": amendment}})
    decisions.append("abort")
    return decisions


def _summarize_patch_changes(changes: dict[str, Any]) -> str:
    lines = []
    for path, change in changes.items():
        if not isinstance(change, dict):
            lines.append(f"? {path}")
            continue
        change_type = change.get("type", "?")
        symbol = {
            "add": "A",
            "delete": "D",
            "update": "M",
        }.get(change_type, "?")
        lines.append(f"{symbol} {path}")
    return "\n".join(lines)


def _normalize_exec_request(raw: dict[str, Any]) -> dict[str, Any]:
    network_approval_context = raw.get("network_approval_context")
    additional_permissions = raw.get("additional_permissions")
    payload: dict[str, Any] = {
        "kind": "network" if network_approval_context else "exec",
        "window_title": "Approval Request",
        "title": _default_title(
            {
                "kind": "network" if network_approval_context else "exec",
                "host": (network_approval_context or {}).get("host"),
            }
        ),
        "message": "Codex needs your approval before continuing.",
        "reason": raw.get("reason"),
        "thread_id": raw.get("thread_id"),
        "thread_label": raw.get("thread_label"),
        "thread": raw.get("thread_label"),
        "is_current_thread": raw.get("is_current_thread"),
        "current_thread_id": raw.get("current_thread_id"),
        "permission_rule": _format_permission_rule(additional_permissions),
        "host": (network_approval_context or {}).get("host"),
        "code": None if network_approval_context else _render_command(raw["command"]),
        "show_shortcuts_hint": False,
        "code_selectable": False,
        "width": DEFAULT_WIDTH,
        "code_height": DEFAULT_CODE_HEIGHT,
        "call_id": raw.get("call_id"),
        "approval_id": raw.get("approval_id"),
        "turn_id": raw.get("turn_id"),
        "cwd": raw.get("cwd"),
        "requester_pid": raw.get("requester_pid"),
        "command": raw.get("command"),
        "protocol_output_type": "exec_approval",
    }
    decisions = _default_exec_decisions(raw)
    payload["options"] = [
        _option_for_exec_decision(
            decision,
            network_approval_context=network_approval_context,
            additional_permissions=additional_permissions,
        )
        for decision in decisions
    ]
    return payload


def _normalize_patch_request(raw: dict[str, Any]) -> dict[str, Any]:
    code = _summarize_patch_changes(raw.get("changes", {}))
    options = [
        {
            "id": "approved",
            "label": "Yes, proceed",
            "key": "y",
            "default": True,
            "decision": "approved",
        },
        {
            "id": "approved_for_session",
            "label": "Yes, and don't ask again for these files",
            "key": "a",
            "decision": "approved_for_session",
        },
        {
            "id": "abort",
            "label": "No, and tell Codex what to do differently",
            "key": "n",
            "cancel": True,
            "decision": "abort",
        },
    ]
    return {
        "kind": "patch",
        "window_title": "Approval Request",
        "title": _default_title({"kind": "patch"}),
        "message": "Codex needs your approval before continuing.",
        "reason": raw.get("reason"),
        "thread_id": raw.get("thread_id"),
        "thread_label": raw.get("thread_label"),
        "thread": raw.get("thread_label"),
        "is_current_thread": raw.get("is_current_thread"),
        "current_thread_id": raw.get("current_thread_id"),
        "permission_rule": f"grant write access under `{raw['grant_root']}`" if raw.get("grant_root") else None,
        "code": code or None,
        "show_shortcuts_hint": False,
        "code_selectable": False,
        "width": DEFAULT_WIDTH,
        "code_height": DEFAULT_CODE_HEIGHT,
        "call_id": raw.get("call_id"),
        "turn_id": raw.get("turn_id"),
        "cwd": raw.get("cwd"),
        "requester_pid": raw.get("requester_pid"),
        "changes": raw.get("changes"),
        "protocol_output_type": "patch_approval",
        "options": options,
    }


def _normalize_elicitation_request(raw: dict[str, Any]) -> dict[str, Any]:
    options = [
        {
            "id": "accept",
            "label": "Yes, provide the requested info",
            "key": "y",
            "default": True,
            "decision": "accept",
        },
        {
            "id": "decline",
            "label": "No, but continue without it",
            "key": "a",
            "decision": "decline",
        },
        {
            "id": "cancel",
            "label": "Cancel this request",
            "key": "n",
            "cancel": True,
            "decision": "cancel",
        },
    ]
    return {
        "kind": "elicitation",
        "window_title": "Approval Request",
        "title": _default_title({"kind": "elicitation", "server_name": raw.get("server_name")}),
        "message": "Codex needs your approval before continuing.",
        "thread_id": raw.get("thread_id"),
        "thread_label": raw.get("thread_label"),
        "thread": raw.get("thread_label"),
        "is_current_thread": raw.get("is_current_thread"),
        "current_thread_id": raw.get("current_thread_id"),
        "server_name": raw.get("server_name"),
        "turn_id": raw.get("turn_id"),
        "cwd": raw.get("cwd"),
        "requester_pid": raw.get("requester_pid"),
        "code": raw.get("message"),
        "show_shortcuts_hint": False,
        "code_selectable": False,
        "width": DEFAULT_WIDTH,
        "code_height": DEFAULT_CODE_HEIGHT,
        "request_id": raw.get("id"),
        "protocol_output_type": "resolve_elicitation",
        "options": options,
    }


def _looks_like_exec_request(raw: dict[str, Any]) -> bool:
    return "call_id" in raw and "command" in raw and "cwd" in raw


def _looks_like_patch_request(raw: dict[str, Any]) -> bool:
    return "call_id" in raw and "changes" in raw and "command" not in raw


def _looks_like_elicitation_request(raw: dict[str, Any]) -> bool:
    return "server_name" in raw and "id" in raw and "message" in raw and "call_id" not in raw


def _default_options(kind: str) -> list[dict[str, Any]]:
    if kind == "patch":
        return [
            {"id": "abort", "label": "No", "key": "n", "cancel": True},
            {
                "id": "approved_for_session",
                "label": "Yes, and don't ask again for these files",
                "key": "a",
            },
            {"id": "approved", "label": "Yes, proceed", "key": "y", "default": True},
        ]
    if kind == "network":
        return [
            {"id": "abort", "label": "No", "key": "n", "cancel": True},
            {
                "id": "approved_for_session",
                "label": "Yes, and allow this host for this conversation",
                "key": "a",
            },
            {"id": "approved", "label": "Yes, just this once", "key": "y", "default": True},
        ]
    if kind == "elicitation":
        return [
            {"id": "cancel", "label": "Cancel this request", "key": "n", "cancel": True},
            {"id": "decline", "label": "No, but continue without it", "key": "a"},
            {"id": "accept", "label": "Yes, provide the requested info", "key": "y", "default": True},
        ]
    return [
        {"id": "abort", "label": "No", "key": "n", "cancel": True},
        {
            "id": "approved_for_session",
            "label": "Yes, and don't ask again this session",
            "key": "a",
        },
        {"id": "approved", "label": "Yes, proceed", "key": "y", "default": True},
    ]


def _default_title(payload: dict[str, Any]) -> str:
    kind = payload["kind"]
    if kind == "patch":
        return "Would you like to make the following edits?"
    if kind == "network":
        host = payload.get("host", "this host")
        return f'Do you want to approve network access to "{host}"?'
    if kind == "elicitation":
        server = payload.get("server_name", "Tool")
        return f"{server} needs your approval."
    return "Would you like to run the following command?"


def _default_message(payload: dict[str, Any]) -> str:
    kind = payload["kind"]
    if kind == "patch":
        return "Codex needs your approval before continuing."
    if kind == "network":
        return "Codex needs your approval before continuing."
    if kind == "elicitation":
        return "Codex needs your approval before continuing."
    return "Codex needs your approval before continuing."


def normalize_payload(raw: dict[str, Any]) -> dict[str, Any]:
    if "requester_pid" not in raw:
        requester_pid = os.environ.get(REQUESTER_PID_ENV)
        if requester_pid:
            raw = dict(raw)
            raw["requester_pid"] = requester_pid
    if _looks_like_exec_request(raw):
        payload = _normalize_exec_request(raw)
    elif _looks_like_patch_request(raw):
        payload = _normalize_patch_request(raw)
    elif _looks_like_elicitation_request(raw):
        payload = _normalize_elicitation_request(raw)
    else:
        payload = dict(raw)
    payload.setdefault("kind", "exec")
    payload.setdefault("window_title", "Approval Request")
    payload.setdefault("title", _default_title(payload))
    payload.setdefault("message", _default_message(payload))
    payload.setdefault("options", _default_options(payload["kind"]))
    payload.setdefault("show_shortcuts_hint", False)
    payload.setdefault("code_selectable", False)
    payload.setdefault("width", DEFAULT_WIDTH)
    payload.setdefault("code_height", DEFAULT_CODE_HEIGHT)

    code = payload.get("code")
    if code is not None and not isinstance(code, str):
        raise ValueError("`code` must be a string when present")

    options = payload["options"]
    if not isinstance(options, list) or not options:
        raise ValueError("`options` must be a non-empty list")

    seen_ids: set[str] = set()
    default_count = 0
    for option in options:
        if not isinstance(option, dict):
            raise ValueError("each option must be an object")
        option.setdefault("id", option["label"])
        if option["id"] in seen_ids:
            raise ValueError(f"duplicate option id: {option['id']}")
        seen_ids.add(option["id"])
        key = option.get("key")
        if key is not None and (not isinstance(key, str) or len(key) != 1):
            raise ValueError(f"option key must be a single character: {option!r}")
        if option.get("default"):
            default_count += 1

    if default_count > 1:
        raise ValueError("at most one option may be marked as default")

    return payload


def build_shortcuts_hint(options: list[dict[str, Any]]) -> str:
    parts = []
    for option in options:
        key = option.get("key")
        if key:
            parts.append(f"{key} = {option['label']}")
    return "Shortcuts: " + ", ".join(parts)


def _wrap_dialog_text(
    text: str,
    *,
    width: int,
    initial_indent: str = "",
    subsequent_indent: str | None = None,
) -> tuple[str, int]:
    if subsequent_indent is None:
        subsequent_indent = " " * len(initial_indent)
    wrapped = textwrap.fill(
        text,
        width=width,
        initial_indent=initial_indent,
        subsequent_indent=subsequent_indent,
    )
    return wrapped, wrapped.count("\n") + 1


def add_display_labels(options: list[dict[str, Any]]) -> None:
    for option in options:
        key = option.get("key")
        if key:
            prefix = f"({key}) "
            display_label = textwrap.fill(
                option["label"],
                width=BUTTON_WRAP_WIDTH,
                initial_indent=prefix,
                subsequent_indent=" " * (len(prefix) + 2),
            )
        else:
            display_label = textwrap.fill(
                option["label"],
                width=BUTTON_WRAP_WIDTH,
                subsequent_indent="    ",
            )
        option["display_label"] = display_label
        option["display_line_count"] = display_label.count("\n") + 1


def add_wrapped_dialog_fields(payload: dict[str, Any]) -> None:
    title_display, title_line_count = _wrap_dialog_text(
        payload["title"],
        width=TITLE_WRAP_WIDTH,
    )
    message_display, message_line_count = _wrap_dialog_text(
        payload["message"],
        width=BODY_WRAP_WIDTH,
    )
    payload["title_display"] = title_display
    payload["title_line_count"] = title_line_count
    payload["message_display"] = message_display
    payload["message_line_count"] = message_line_count

    detail_specs: list[tuple[str, str | None]] = [
        ("Thread: ", payload.get("thread")),
        (
            "Requester PID: ",
            str(payload["requester_pid"]) if payload.get("requester_pid") is not None else None,
        ),
        ("Working dir: ", payload.get("cwd")),
        ("Reason: ", payload.get("reason")),
        ("Permission rule: ", payload.get("permission_rule")),
        ("Host: ", payload.get("host") if payload.get("kind") == "network" else None),
        (
            "Server: ",
            payload.get("server_name") if payload.get("kind") == "elicitation" else None,
        ),
    ]
    detail_rows = []
    for prefix, value in detail_specs:
        if not value:
            continue
        text, line_count = _wrap_dialog_text(
            f"{prefix}{value}",
            width=BODY_WRAP_WIDTH,
            subsequent_indent=" " * len(prefix),
        )
        detail_rows.append({"text": text, "line_count": line_count})
    payload["detail_rows"] = detail_rows

    if payload.get("show_shortcuts_hint"):
        shortcuts_display, shortcuts_line_count = _wrap_dialog_text(
            payload["shortcuts_hint"],
            width=BODY_WRAP_WIDTH,
        )
        payload["shortcuts_display"] = shortcuts_display
        payload["shortcuts_line_count"] = shortcuts_line_count


def format_thread_summary(payload: dict[str, Any]) -> str | None:
    thread_label = payload.get("thread_label") or payload.get("thread")
    thread_id = payload.get("thread_id")
    is_current_thread = payload.get("is_current_thread")
    current_thread_id = payload.get("current_thread_id")

    if is_current_thread is None and thread_id is not None and current_thread_id is not None:
        is_current_thread = thread_id == current_thread_id

    if not thread_label and not thread_id:
        return None

    current_suffix = " (current)" if is_current_thread else ""
    if thread_label and thread_id:
        return f"{thread_label}{current_suffix}  {thread_id}"
    if thread_label:
        return f"{thread_label}{current_suffix}"
    return str(thread_id)


def build_jxa(payload: dict[str, Any]) -> str:
    payload_json = json.dumps(payload)
    return f"""
ObjC.import("AppKit");
ObjC.import("Foundation");

function nsstr(value) {{
  return $(value);
}}

function makeLabel(text, width, font) {{
  var field = $.NSTextField.alloc.initWithFrame($.NSMakeRect(0, 0, width, 22));
  field.setStringValue(nsstr(text));
  field.setEditable(false);
  field.setSelectable(false);
  field.setBezeled(false);
  field.setBordered(false);
  field.setDrawsBackground(false);
  field.setLineBreakMode($.NSLineBreakByWordWrapping);
  field.setUsesSingleLineMode(false);
  field.setFont(font);
  return field;
}}

function measureWrappedTextHeight(text, width, font) {{
  var field = makeLabel(text, width, font);
  field.setFrame($.NSMakeRect(0, 0, width, 1000000));
  var cellSize = field.cell.cellSizeForBounds($.NSMakeRect(0, 0, width, 1000000));
  return Math.ceil(cellSize.height);
}}

var payload = {payload_json};
var app = Application.currentApplication();
app.includeStandardAdditions = true;

var nsApp = $.NSApplication.sharedApplication;
nsApp.setActivationPolicy($.NSApplicationActivationPolicyRegular);
$.NSRunningApplication.currentApplication.activateWithOptions(
  $.NSApplicationActivateIgnoringOtherApps
);
nsApp.activateIgnoringOtherApps(true);
delay(0.25);

var margin = 18;
var innerWidth = payload.width - (margin * 2);
var titleFont = $.NSFont.boldSystemFontOfSize(16);
var bodyFont = $.NSFont.systemFontOfSize(13);
var buttonFont = $.NSFont.systemFontOfSize(13);
var buttonHorizontalInset = 14;
var buttonVerticalInset = 7;

var titleHeight = Math.max(22, (payload.title_line_count || 1) * 20);
var messageHeight = Math.max(18, (payload.message_line_count || 1) * 17);
var detailsHeight = 0;
for (var i = 0; i < payload.detail_rows.length; i++) {{
  detailsHeight += Math.max(18, payload.detail_rows[i].line_count * 17) + 6;
}}
var shortcutsHeight = payload.show_shortcuts_hint
  ? (Math.max(18, (payload.shortcuts_line_count || 1) * 17) + 8)
  : 0;
var codeHeight = payload.code ? (payload.code_height + 10) : 0;
var buttonTextWidth = Math.max(80, innerWidth - (buttonHorizontalInset * 2));
var buttonHeights = [];
var buttonsBlockHeight = 0;
for (var i = 0; i < payload.options.length; i++) {{
  var label = payload.options[i].display_label || payload.options[i].label;
  var textHeight = Math.max(18, measureWrappedTextHeight(label, buttonTextWidth, buttonFont));
  var buttonHeight = Math.max(28, textHeight + (buttonVerticalInset * 2));
  buttonHeights.push(buttonHeight);
  buttonsBlockHeight += buttonHeight;
  if (i > 0) {{
    buttonsBlockHeight += {BUTTON_VERTICAL_GAP};
  }}
}}
var contentHeight =
  margin +
  titleHeight +
  8 +
  messageHeight +
  12 +
  detailsHeight +
  (payload.code ? codeHeight + 12 : 0) +
  shortcutsHeight +
  buttonsBlockHeight +
  margin;

var selection = null;
var cancelIndex = payload.options.findIndex(function(option) {{
  return Boolean(option.cancel);
}});

ObjC.registerSubclass({{
  name: "CodexApprovalDialogController",
  methods: {{
    "buttonPressed:": {{
      types: ["void", ["id"]],
      implementation: function(sender) {{
        selection = Number(sender.tag);
        $.NSApp.stopModalWithCode(1000 + selection);
        sender.window.orderOut(null);
      }}
    }}
  }}
}});

var controller = $.CodexApprovalDialogController.alloc.init;

var win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
  $.NSMakeRect(0, 0, payload.width, contentHeight),
  $.NSWindowStyleMaskTitled,
  $.NSBackingStoreBuffered,
  false
);
win.setTitle(nsstr(payload.window_title));
win.setOpaque(true);
win.setAlphaValue(1.0);
win.setBackgroundColor($.NSColor.windowBackgroundColor);
win.setTitlebarAppearsTransparent(false);
win.setMovableByWindowBackground(false);
win.setLevel($.NSModalPanelWindowLevel);
var screen = $.NSScreen.mainScreen;
if (screen) {{
  var visibleFrame = screen.visibleFrame;
  var originX = visibleFrame.origin.x + Math.max(0, (visibleFrame.size.width - payload.width) / 2);
  var originY = visibleFrame.origin.y + Math.max(0, (visibleFrame.size.height - contentHeight) / 2);
  win.setFrameOrigin($.NSMakePoint(originX, originY));
}}

var contentView = win.contentView;
var y = contentHeight - margin;

var titleField = makeLabel(payload.title_display || payload.title, innerWidth, titleFont);
y -= titleHeight;
titleField.setFrame($.NSMakeRect(margin, y, innerWidth, titleHeight));
contentView.addSubview(titleField);
y -= 8;

var messageField = makeLabel(payload.message_display || payload.message, innerWidth, bodyFont);
y -= messageHeight;
messageField.setFrame($.NSMakeRect(margin, y, innerWidth, messageHeight));
contentView.addSubview(messageField);
y -= 12;

for (var i = 0; i < payload.detail_rows.length; i++) {{
  var rowHeight = Math.max(18, payload.detail_rows[i].line_count * 17);
  var rowField = makeLabel(payload.detail_rows[i].text, innerWidth, bodyFont);
  y -= rowHeight;
  rowField.setFrame($.NSMakeRect(margin, y, innerWidth, rowHeight));
  contentView.addSubview(rowField);
  y -= 6;
}}

if (payload.code) {{
  var scrollY = y - payload.code_height;
  var textView = $.NSTextView.alloc.initWithFrame(
    $.NSMakeRect(0, 0, innerWidth, payload.code_height)
  );
  textView.setEditable(false);
  textView.setSelectable(Boolean(payload.code_selectable));
  textView.setRichText(false);
  textView.setImportsGraphics(false);
  textView.setUsesFindBar(true);
  textView.setFont($.NSFont.userFixedPitchFontOfSize(12));
  textView.textContainer.setWidthTracksTextView(true);
  textView.textContainer.setContainerSize($.NSMakeSize(innerWidth, 10000000));
  textView.setHorizontallyResizable(false);
  textView.setVerticallyResizable(true);
  textView.setMaxSize($.NSMakeSize(innerWidth, 10000000));
  textView.setString(nsstr(payload.code));

  var scrollView = $.NSScrollView.alloc.initWithFrame(
    $.NSMakeRect(margin, scrollY, innerWidth, payload.code_height)
  );
  scrollView.setBorderType($.NSBezelBorder);
  scrollView.setHasVerticalScroller(true);
  scrollView.setHasHorizontalScroller(false);
  scrollView.setAutohidesScrollers(true);
  scrollView.setDocumentView(textView);
  contentView.addSubview(scrollView);
  y = scrollY - 12;
}}

if (payload.show_shortcuts_hint) {{
  var hintHeight = Math.max(18, (payload.shortcuts_line_count || 1) * 17);
  var hintField = makeLabel(payload.shortcuts_display || payload.shortcuts_hint, innerWidth, bodyFont);
  y -= hintHeight;
  hintField.setFrame($.NSMakeRect(margin, y, innerWidth, hintHeight));
  contentView.addSubview(hintField);
  y -= 8;
}}

var defaultIndex = payload.options.findIndex(function(option) {{
  return Boolean(option.default);
}});
var buttons = [];
for (var i = 0; i < payload.options.length; i++) {{
  var option = payload.options[i];
  var buttonHeight = buttonHeights[i];
  var labelText = option.display_label || option.label;
  var labelHeight = Math.max(18, measureWrappedTextHeight(labelText, buttonTextWidth, buttonFont));
  y -= buttonHeight;
  var button = $.NSButton.alloc.initWithFrame($.NSMakeRect(margin, y, innerWidth, buttonHeight));
  button.setTitle(nsstr(""));
  button.setTag(i);
  button.setTarget(controller);
  button.setAction("buttonPressed:");
  button.setBordered(false);
  button.setWantsLayer(true);
  button.layer.setCornerRadius(7);
  if (i === defaultIndex) {{
    button.layer.setBackgroundColor($.NSColor.controlAccentColor.CGColor);
  }} else {{
    button.layer.setBackgroundColor($.NSColor.controlColor.CGColor);
    button.layer.setBorderWidth(1);
    button.layer.setBorderColor($.NSColor.separatorColor.CGColor);
  }}
  button.setFont(buttonFont);
  var buttonLabel = makeLabel(labelText, buttonTextWidth, buttonFont);
  if (i === defaultIndex) {{
    buttonLabel.setTextColor($.NSColor.alternateSelectedControlTextColor);
  }}
  buttonLabel.setFrame(
    $.NSMakeRect(
      buttonHorizontalInset,
      Math.max(buttonVerticalInset, Math.floor((buttonHeight - labelHeight) / 2)),
      buttonTextWidth,
      labelHeight
    )
  );
  button.addSubview(buttonLabel);
  if (option.key) {{
    button.setKeyEquivalent(nsstr(option.key));
    button.setKeyEquivalentModifierMask(0);
  }}
  if (i === defaultIndex) {{
    win.setDefaultButtonCell(button.cell);
  }}
  buttons.push(button);
  contentView.addSubview(button);
  if (i + 1 < payload.options.length) {{
    y -= {BUTTON_VERTICAL_GAP};
  }}
}}

for (var i = 0; i < payload.options.length; i++) {{
  var option = payload.options[i];
  if (option.key) {{
    buttons[i].setKeyEquivalent(nsstr(option.key));
    buttons[i].setKeyEquivalentModifierMask(0);
  }}
}}

win.makeKeyAndOrderFront(null);
$.NSRunningApplication.currentApplication.activateWithOptions(
  $.NSApplicationActivateIgnoringOtherApps | $.NSApplicationActivateAllWindows
);
nsApp.activateIgnoringOtherApps(true);

var response = $.NSApp.runModalForWindow(win);
var responseIndex = selection;
if (responseIndex === null || responseIndex < 0) {{
  responseIndex = cancelIndex >= 0 ? cancelIndex : defaultIndex;
}}
var option = payload.options[responseIndex];
var result = JSON.stringify({{
  thread_id: payload.thread_id || null,
  thread_label: payload.thread_label || null,
  call_id: payload.call_id || null,
  approval_id: payload.approval_id || null,
  turn_id: payload.turn_id || null,
  id: option.id,
  label: option.label,
  key: option.key || null,
  decision: option.decision || null,
  response_index: responseIndex,
  response_code: response
}}) + "\\n";
$.NSFileHandle.fileHandleWithStandardOutput.writeData(
  nsstr(result).dataUsingEncoding($.NSUTF8StringEncoding)
);
""".strip()


def build_simple_jxa(payload: dict[str, Any]) -> str:
    buttons = [option["label"] for option in payload["options"]]
    default_button = next(
        (option["label"] for option in payload["options"] if option.get("default")),
        buttons[-1],
    )
    cancel_button = next(
        (option["label"] for option in payload["options"] if option.get("cancel")),
        None,
    )

    message_lines = [payload["message"]]
    if payload.get("thread"):
        message_lines.append(f"Thread: {payload['thread']}")
    if payload.get("requester_pid"):
        message_lines.append(f"Requester PID: {payload['requester_pid']}")
    if payload.get("cwd"):
        message_lines.append(f"Working dir: {payload['cwd']}")
    if payload.get("reason"):
        message_lines.append(f"Reason: {payload['reason']}")
    if payload.get("permission_rule"):
        message_lines.append(f"Permission rule: {payload['permission_rule']}")
    if payload.get("host") and payload.get("kind") == "network":
        message_lines.append(f"Host: {payload['host']}")
    if payload.get("server_name") and payload.get("kind") == "elicitation":
        message_lines.append(f"Server: {payload['server_name']}")
    if payload.get("code"):
        message_lines.append("")
        message_lines.append(payload["code"])

    payload_json = json.dumps(payload)
    dialog_args = {
        "withTitle": payload["title"],
        "buttons": buttons,
        "defaultButton": default_button,
    }
    if cancel_button is not None:
        dialog_args["cancelButton"] = cancel_button
    dialog_args_json = json.dumps(dialog_args)
    message = "\n".join(message_lines)

    return f"""
var payload = {payload_json};
var dialogArgs = {dialog_args_json};
var app = Application.currentApplication();
app.includeStandardAdditions = true;
app.activate();

var response = app.displayDialog({json.dumps(message)}, dialogArgs);
var button = response.buttonReturned();
var selected = null;
for (var i = 0; i < payload.options.length; i++) {{
  if (payload.options[i].label === button) {{
    selected = payload.options[i];
    selected.response_index = i;
    selected.response_code = String(1000 + i);
    break;
  }}
}}
if (selected === null) {{
  throw new Error("unknown dialog button returned: " + button);
}}

var result = JSON.stringify(selected) + "\\n";
$.NSFileHandle.fileHandleWithStandardOutput.writeData(
  $(result).dataUsingEncoding($.NSUTF8StringEncoding)
);
""".strip()


def _debug_dir() -> Path | None:
    debug_dir = os.environ.get(DEBUG_DIR_ENV)
    debug_enabled = os.environ.get(DEBUG_ENV)
    if debug_dir is None and debug_enabled != "1":
        return None
    base = Path(debug_dir) if debug_dir else Path(tempfile.gettempdir()) / "codex-approval-dialog"
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    path = base / f"{stamp}-{os.getpid()}"
    path.mkdir(parents=True, exist_ok=True)
    return path


def _write_debug_file(debug_dir: Path | None, name: str, content: str) -> None:
    if debug_dir is None:
        return
    (debug_dir / name).write_text(content, encoding="utf-8")


def _append_debug_event(debug_dir: Path | None, message: str) -> None:
    if debug_dir is None:
        return
    timestamp = datetime.now().isoformat(timespec="milliseconds")
    with (debug_dir / "timeline.log").open("a", encoding="utf-8") as handle:
        handle.write(f"{timestamp} {message}\n")


def _read_stdin_text(debug_dir: Path | None) -> str:
    stdin_fd = sys.stdin.fileno()
    chunks: list[bytes] = []
    total_bytes = 0
    timeout_ms = max(int(os.environ.get(STDIN_TIMEOUT_ENV, DEFAULT_STDIN_TIMEOUT_MS)), 1)
    _append_debug_event(debug_dir, "load_input:stdin_read:start")
    while True:
        readable, _, _ = select.select([stdin_fd], [], [], timeout_ms / 1000.0)
        if not readable:
            _append_debug_event(
                debug_dir,
                f"load_input:stdin_read:timeout total_bytes={total_bytes} timeout_ms={timeout_ms}",
            )
            if total_bytes > 0:
                _append_debug_event(
                    debug_dir,
                    f"load_input:stdin_read:progress_after_timeout total_bytes={total_bytes}",
                )
                break
            raise TimeoutError(
                f"stdin read timed out after {timeout_ms} ms waiting for input"
            )
        chunk = os.read(stdin_fd, 65536)
        if not chunk:
            _append_debug_event(
                debug_dir,
                f"load_input:stdin_read:eof total_bytes={total_bytes}",
            )
            break
        nul_index = chunk.find(b"\0")
        if nul_index >= 0:
            chunk = chunk[:nul_index]
            if chunk:
                chunks.append(chunk)
                total_bytes += len(chunk)
            _append_debug_event(
                debug_dir,
                f"load_input:stdin_read:nul_terminator total_bytes={total_bytes}",
            )
            break
        chunks.append(chunk)
        total_bytes += len(chunk)
        _append_debug_event(
            debug_dir,
            f"load_input:stdin_read:chunk bytes={len(chunk)} total_bytes={total_bytes}",
        )
    return b"".join(chunks).decode("utf-8")


def run_dialog(payload: dict[str, Any], debug_dir: Path | None) -> dict[str, Any]:
    _append_debug_event(debug_dir, "run_dialog:start")
    raw_mode = "simple" if os.environ.get(SIMPLE_DIALOG_ENV) else "nsalert"
    script = build_simple_jxa(payload) if raw_mode == "simple" else build_jxa(payload)
    _write_debug_file(debug_dir, "normalized-payload.json", json.dumps(payload, indent=2))
    _write_debug_file(debug_dir, "dialog-mode.txt", raw_mode + "\n")
    _append_debug_event(debug_dir, f"run_dialog:mode={raw_mode}")
    temp_path: Path | None = None
    try:
        if debug_dir is not None:
            temp_path = debug_dir / "dialog.js"
            temp_path.write_text(script, encoding="utf-8")
        else:
            with tempfile.NamedTemporaryFile(
                mode="w", suffix=".js", prefix="codex_approval_", delete=False
            ) as temp_file:
                temp_file.write(script)
                temp_path = Path(temp_file.name)

        command = ["osascript", "-l", "JavaScript", str(temp_path)]
        _write_debug_file(debug_dir, "osascript-command.txt", " ".join(command) + "\n")
        _write_debug_file(debug_dir, "run-status.txt", "starting\n")
        _append_debug_event(debug_dir, f"run_dialog:command={' '.join(command)}")

        timeout_ms = os.environ.get(OSASCRIPT_TIMEOUT_ENV)
        timeout = None
        if timeout_ms:
            timeout = max(int(timeout_ms), 1) / 1000.0
        _append_debug_event(debug_dir, f"run_dialog:timeout={timeout!r}")

        try:
            _append_debug_event(debug_dir, "run_dialog:subprocess_run:start")
            completed = subprocess.run(
                command,
                capture_output=True,
                check=False,
                text=True,
                timeout=timeout,
            )
            _append_debug_event(
                debug_dir,
                f"run_dialog:subprocess_run:done returncode={completed.returncode}",
            )
        except subprocess.TimeoutExpired as exc:
            _write_debug_file(
                debug_dir,
                "osascript-timeout.txt",
                f"timeout_seconds={exc.timeout}\nstdout={exc.stdout or ''}\nstderr={exc.stderr or ''}\n",
            )
            _append_debug_event(debug_dir, f"run_dialog:timeout_expired seconds={exc.timeout}")
            raise RuntimeError(f"osascript timed out after {exc.timeout} seconds") from exc
    finally:
        if temp_path is not None and debug_dir is None:
            temp_path.unlink(missing_ok=True)

    _write_debug_file(debug_dir, "osascript-stdout.txt", completed.stdout)
    _write_debug_file(debug_dir, "osascript-stderr.txt", completed.stderr)
    _write_debug_file(debug_dir, "osascript-returncode.txt", f"{completed.returncode}\n")
    _write_debug_file(debug_dir, "run-status.txt", "completed\n")
    _append_debug_event(debug_dir, "run_dialog:completed")

    if completed.returncode != 0:
        stderr = completed.stderr.strip()
        stdout = completed.stdout.strip()
        _append_debug_event(debug_dir, "run_dialog:error:nonzero_return")
        raise RuntimeError(stderr or stdout or "osascript failed")

    stdout = completed.stdout.strip()
    if not stdout:
        _append_debug_event(debug_dir, "run_dialog:error:no_stdout")
        raise RuntimeError("osascript returned no output")
    _append_debug_event(debug_dir, "run_dialog:parsing_stdout_json")
    return json.loads(stdout)


def build_protocol_output(payload: dict[str, Any], selection: dict[str, Any]) -> dict[str, Any]:
    protocol_output_type = payload.get("protocol_output_type")
    decision = selection.get("decision")

    if protocol_output_type == "exec_approval":
        approval_id = payload.get("approval_id") or payload.get("call_id")
        if not approval_id:
            raise ValueError("exec approval payload is missing approval_id/call_id")
        result = {
            "type": "exec_approval",
            "id": approval_id,
            "decision": decision,
        }
        turn_id = payload.get("turn_id")
        if turn_id is not None:
            result["turn_id"] = turn_id
        return result

    if protocol_output_type == "patch_approval":
        call_id = payload.get("call_id")
        if not call_id:
            raise ValueError("patch approval payload is missing call_id")
        return {
            "type": "patch_approval",
            "id": call_id,
            "decision": decision,
        }

    if protocol_output_type == "resolve_elicitation":
        server_name = payload.get("server_name")
        request_id = payload.get("request_id")
        if server_name is None or request_id is None:
            raise ValueError("elicitation payload is missing server_name or request_id")
        return {
            "type": "resolve_elicitation",
            "server_name": server_name,
            "request_id": request_id,
            "decision": decision,
        }

    return selection


def build_test_payload() -> dict[str, Any]:
    return {
        "kind": "exec",
        "window_title": "Approval Request",
        "title": "Would you like to run the following command?",
        "message": "Codex needs your approval before continuing.",
        "thread_label": "Main [default]",
        "thread_id": "test-thread-id",
        "is_current_thread": True,
        "reason": "Smoke-test the custom macOS approval dialog renderer.",
        "cwd": "/Users/ebrevdo/code/codex",
        "code": (
            "python -m pytest tests/test_example.py "
            "--maxfail=1 -q --some-very-long-flag=value "
            "--another-long-flag='wrapped command text should stay inside the code box'"
        ),
        "protocol_output_type": "exec_approval",
        "approval_id": "test-approval-id",
        "turn_id": "test-turn-id",
        "options": [
            {
                "id": "approved",
                "label": "Yes, proceed",
                "key": "y",
                "default": True,
                "decision": "approved",
            },
            {
                "id": "approved_for_session",
                "label": "Yes, and do not ask again for this exact command during this session even if it is requested repeatedly from the same approval flow or a closely related one",
                "key": "a",
                "decision": "approved_for_session",
            },
            {
                "id": "abort",
                "label": "No, and tell Codex what to do differently",
                "key": "n",
                "cancel": True,
                "decision": "abort",
            },
        ],
    }


def load_input(path: str | None, *, test_mode: bool, debug_dir: Path | None) -> dict[str, Any]:
    if path:
        return json.loads(Path(path).read_text())
    if test_mode:
        if sys.stdin.isatty():
            return build_test_payload()
        readable, _, _ = select.select([sys.stdin], [], [], 0)
        if not readable:
            return build_test_payload()
    raw_text = _read_stdin_text(debug_dir)
    if test_mode and not raw_text.strip():
        return build_test_payload()
    return json.loads(raw_text)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", help="Path to the JSON request payload")
    parser.add_argument(
        "--print-jxa",
        action="store_true",
        help="Print the generated JXA script and exit without running it",
    )
    parser.add_argument(
        "--normalize-only",
        action="store_true",
        help="Print the normalized request JSON and exit without running it",
    )
    parser.add_argument(
        "--test",
        action="store_true",
        help="Run a dialog smoke test; if no input is provided on stdin or via --input, use a built-in sample payload",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    debug_dir = _debug_dir()
    try:
        _append_debug_event(debug_dir, "main:start")
        raw = load_input(args.input, test_mode=args.test, debug_dir=debug_dir)
        _append_debug_event(debug_dir, "main:input_loaded")
        _write_debug_file(debug_dir, "raw-input.json", json.dumps(raw, indent=2))
        payload = normalize_payload(raw)
        _append_debug_event(debug_dir, "main:payload_normalized")
        payload["thread"] = format_thread_summary(payload)
        add_display_labels(payload["options"])
        payload["shortcuts_hint"] = build_shortcuts_hint(payload["options"])
        add_wrapped_dialog_fields(payload)
        if args.normalize_only:
            _append_debug_event(debug_dir, "main:normalize_only")
            json.dump(payload, sys.stdout, indent=2)
            sys.stdout.write("\n")
            return 0
        if args.print_jxa:
            _append_debug_event(debug_dir, "main:print_jxa")
            jxa = build_simple_jxa(payload) if os.environ.get(SIMPLE_DIALOG_ENV) else build_jxa(payload)
            sys.stdout.write(jxa)
            sys.stdout.write("\n")
            return 0
        _append_debug_event(debug_dir, "main:run_dialog")
        selection = run_dialog(payload, debug_dir)
        _append_debug_event(debug_dir, "main:dialog_selection_received")
        protocol_output = build_protocol_output(payload, selection)
        result = (
            {"selection": selection, "protocol_output": protocol_output}
            if args.test
            else protocol_output
        )
        _append_debug_event(debug_dir, "main:protocol_output_built")
    except Exception as exc:  # pragma: no cover - CLI error path
        _append_debug_event(debug_dir, f"main:error {exc}")
        _write_debug_file(debug_dir, "python-error.txt", f"{exc}\n")
        json.dump({"error": str(exc)}, sys.stderr)
        sys.stderr.write("\n")
        return 1

    _append_debug_event(debug_dir, "main:success")
    json.dump(result, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
