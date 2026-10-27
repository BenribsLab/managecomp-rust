#!/usr/bin/env bash
# Captures web de la documentation : voir tools/doc_captures_web.mjs.
exec node "$(dirname "$0")/doc_captures_web.mjs"
