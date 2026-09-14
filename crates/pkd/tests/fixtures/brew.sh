#!/bin/bash
# Synthetic manager. All mutable state stays in the test HOME.
set -euo pipefail
[[ -z ${LD_PRELOAD+x} && $HOMEBREW_NO_AUTO_UPDATE == 1 && $HOMEBREW_NO_INSTALL_CLEANUP == 1 ]]
if [[ -f $HOME/fail ]]; then
    echo 'synthetic native failure' >&2
    exit 1
fi
state=$(/bin/cat "$HOME/state.json" 2>/dev/null || echo '{"installed":null,"candidate":"1.0"}')
case "$1" in
    --prefix) echo "$HOME" ;;
    formulae) echo fixture ;;
    info)
        echo info >>"$HOME/queries.log"
        /usr/bin/jq -n --argjson s "$state" --arg args "$*" '{formulae: (if ($args|contains("--installed")) and $s.installed == null then [] else [{full_name:"fixture",desc:"Synthetic fixture",homepage:"",versions:{stable:$s.candidate},revision:0,installed:(if $s.installed then [{version:$s.installed}] else [] end),outdated:($s.installed != null and $s.installed != $s.candidate),dependencies:[]}] end)}'
        ;;
    update | install | upgrade | uninstall)
        case "$1" in
            update)
                [[ $# == 1 ]]
                filter='.candidate="2.0"'
                ;;
            uninstall)
                [[ "$*" == 'uninstall --formula --force -- fixture' ]]
                filter='.installed=null'
                ;;
            *)
                [[ "${*:2}" == '--formula -- fixture' ]]
                filter='.installed=.candidate'
                ;;
        esac
        /usr/bin/jq "$filter" <<<"$state" >"$HOME/state.json"
        echo 'synthetic native progress'
        ;;
    *) exit 2 ;;
esac
