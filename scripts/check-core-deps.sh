#!/usr/bin/env bash
# Fails when cerata_core or cerata_markdown pulls any `gpui*` crate, on any
# target and through any edge kind (normal, build, dev). Both crates must stay
# usable without a window: pure tests now, an Android UI later (PRD hard
# constraint, PLAN.md section 3.7).
set -euo pipefail

status=0
for crate in cerata_core cerata_markdown; do
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
