#!/usr/bin/env bash
set -euo pipefail

# xChat Automated Sandbox Driver & Gate 1 Validator
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

case "${1:-proof}" in
    up)
        make -C "${SCRIPT_DIR}" up
        ;;
    down)
        make -C "${SCRIPT_DIR}" down
        ;;
    reset)
        make -C "${SCRIPT_DIR}" reset
        ;;
    kill-switch)
        make -C "${SCRIPT_DIR}" kill-switch
        ;;
    proof)
        make -C "${SCRIPT_DIR}" proof
        ;;
    *)
        echo "Usage: $0 {up|down|reset|kill-switch|proof}"
        exit 1
        ;;
esac
