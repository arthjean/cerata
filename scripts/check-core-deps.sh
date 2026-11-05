#!/usr/bin/env bash
set -euo pipefail

status=0
for crate in cerata_core cerata_markdown cerata_render; do
    offenders=$(cargo tree --locked -p "$crate" --target all -e normal,build,dev \
        --prefix none --format '{p}' | awk '{print $1}' | grep -E '^gpui' | sort -u || true)
    if [ -n "$offenders" ]; then
        for offender in $offenders; do
            echo "error: $crate depends on GUI crate '$offender'; $crate must not depend on gpui*" >&2
        done
        status=1
    else
        echo "ok: $crate has no gpui* dependency"
    fi
done
exit "$status"
