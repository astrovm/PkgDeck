#!/bin/bash
set -euo pipefail
case "$1" in
    --prefix) echo "$HOME" ;;
    info)
        if [[ -f $HOME/query-fails ]]; then
            echo 'synthetic query failure' >&2
            exit 1
        fi
        /usr/bin/jq --arg args "$*" '{formulae:[to_entries[] | . as $entry | select(($args|contains("--installed")) or ($args|contains($entry.key))) | {full_name:.key,desc:"Synthetic batch fixture",homepage:"",versions:{stable:"2"},revision:0,installed:[{version:.value}],outdated:(.value!="2"),dependencies:[]}]}' "$HOME/state.json"
        ;;
    upgrade)
        if [[ $# == 2 && $2 == --formula ]]; then
            if [[ -f $HOME/fail ]]; then
                echo 'synthetic upgrade failure' >&2
                exit 1
            fi
            echo all >>"$HOME/attempts"
            if [[ -f $HOME/slow ]]; then
                : >"$HOME/started"
                /bin/sleep 2
            fi
            /usr/bin/jq 'with_entries(if .value != "2" then .value = "2" else . end)' "$HOME/state.json" >"$HOME/state.tmp"
            /bin/mv "$HOME/state.tmp" "$HOME/state.json"
            exit 0
        fi
        [[ $# == 4 && $2 == --formula && $3 == -- ]]
        name=$4
        /usr/bin/jq -e --arg n "$name" 'has($n) and .[$n] != "2"' "$HOME/state.json" >/dev/null
        echo "$name" >>"$HOME/attempts"
        if [[ -f $HOME/slow ]]; then
            : >"$HOME/started"
            /bin/sleep 2
        fi
        if [[ $name == fixture-b && -f $HOME/fail ]]; then
            echo 'synthetic upgrade failure' >&2
            exit 1
        fi
        /usr/bin/jq --arg n "$name" '.[$n]="2"' "$HOME/state.json" >"$HOME/state.tmp"
        /bin/mv "$HOME/state.tmp" "$HOME/state.json"
        ;;
    *) exit 2 ;;
esac
