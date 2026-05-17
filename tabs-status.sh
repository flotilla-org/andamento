#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  tabs-status.sh status [--pane terminal:ID] --priority PRIORITY --title TITLE [--detail TEXT] [--icon-png PATH]
  tabs-status.sh clear [--pane terminal:ID]

Examples:
  tabs-status.sh status --priority waiting --title "Claude waiting" --detail "needs input" --icon-png ./waiting.png
  tabs-status.sh status --pane plugin:7 --priority error --title "tests failed" --icon-png ./failed.png
  tabs-status.sh clear --pane terminal:1

If --pane is omitted, tabs-status.sh uses $ZELLIJ_PANE_ID or $ZELLIJ_PANE.
Priorities: idle, info, success, waiting, warning, error
EOF
}

json_escape() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  printf '%s' "$s"
}

json_string_or_null() {
  if [[ $# -eq 0 || -z ${1-} ]]; then
    printf 'null'
  else
    printf '"%s"' "$(json_escape "$1")"
  fi
}

zellij_bin() {
  if [[ -n ${ZELLIJ_BIN:-} ]]; then
    printf '%s' "$ZELLIJ_BIN"
  elif [[ -x /Users/robert/dev/zellij/target/dev-opt/zellij ]]; then
    printf '%s' /Users/robert/dev/zellij/target/dev-opt/zellij
  else
    printf '%s' zellij
  fi
}

json_icon_or_null() {
  if [[ $# -eq 0 || -z ${1-} ]]; then
    printf 'null'
  else
    printf '{"kind":"png-file","value":"%s"}' "$(json_escape "$1")"
  fi
}

canonical_file_path() {
  local path=$1
  if [[ ! -f $path ]]; then
    echo "Icon PNG does not exist or is not a file: $path" >&2
    exit 2
  fi
  local dir
  local file
  dir=$(dirname -- "$path")
  file=$(basename -- "$path")
  printf '%s/%s' "$(cd "$dir" && pwd -P)" "$file"
}

parse_pane() {
  local pane=$1
  case "$pane" in
    terminal:[0-9]*)
      pane_kind=terminal
      pane_id=${pane#terminal:}
      ;;
    plugin:[0-9]*)
      pane_kind=plugin
      pane_id=${pane#plugin:}
      ;;
    *)
      echo "Invalid pane '$pane'. Use terminal:ID or plugin:ID." >&2
      exit 2
      ;;
  esac
}

dry_run=0

if [[ ${1-} == "-h" || ${1-} == "--help" ]]; then
  usage
  exit 0
fi

command=${1-}
shift || true

pane="${ZELLIJ_PANE_ID:-${ZELLIJ_PANE:-}}"
priority=""
title=""
detail=""
icon_png=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --pane)
      pane=${2-}
      shift 2
      ;;
    --priority)
      priority=${2-}
      shift 2
      ;;
    --title)
      title=${2-}
      shift 2
      ;;
    --detail)
      detail=${2-}
      shift 2
      ;;
    --icon-png)
      icon_png=${2-}
      shift 2
      ;;
    --icon)
      echo "--icon was removed because status icons are image assets. Use --icon-png PATH." >&2
      exit 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z $command ]]; then
  usage >&2
  exit 2
fi

if [[ -z $pane ]]; then
  echo "No --pane provided and neither ZELLIJ_PANE_ID nor ZELLIJ_PANE is set." >&2
  exit 2
fi

if [[ $pane =~ ^[0-9]+$ ]]; then
  pane="terminal:$pane"
fi

parse_pane "$pane"

case "$command" in
  status)
    if [[ -z $priority || -z $title ]]; then
      echo "status requires --priority and --title" >&2
      exit 2
    fi
    if [[ -n $icon_png ]]; then
      icon_png=$(canonical_file_path "$icon_png")
    fi
    payload=$(
      printf '{"type":"set-pane-status","pane_id":{"kind":"%s","id":%s},"priority":"%s","title":"%s","detail":%s,"icon":%s,"timestamp_ms":null}' \
        "$pane_kind" \
        "$pane_id" \
        "$(json_escape "$priority")" \
        "$(json_escape "$title")" \
        "$(json_string_or_null "$detail")" \
        "$(json_icon_or_null "$icon_png")"
    )
    if [[ $dry_run -eq 1 ]]; then
      printf '%s\n' "$payload"
    else
      "$(zellij_bin)" pipe --name tabs-set-pane-status -- "$payload"
    fi
    ;;
  clear)
    payload=$(printf '{"type":"clear-pane-status","pane_id":{"kind":"%s","id":%s}}' "$pane_kind" "$pane_id")
    if [[ $dry_run -eq 1 ]]; then
      printf '%s\n' "$payload"
    else
      "$(zellij_bin)" pipe --name tabs-clear-pane-status -- "$payload"
    fi
    ;;
  *)
    echo "Unknown command: $command" >&2
    usage >&2
    exit 2
    ;;
esac
