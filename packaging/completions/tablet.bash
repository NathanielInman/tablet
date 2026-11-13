# bash completion for tablet
_tablet_profiles() {
    sed -n 's/^[[:space:]]*name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
        "${TABLET_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/tablet}/config.toml" 2>/dev/null
}

_tablet() {
    local cur=${COMP_WORDS[COMP_CWORD]}
    local commands="connect disconnect reconnect flip ack signin check why need status history doctor toggle show hide bar daemon restart help --version"
    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$commands" -- "$cur"))
        return
    fi
    case ${COMP_WORDS[1]} in
        connect | reconnect | ack | signin | check) COMPREPLY=($(compgen -W "$(_tablet_profiles)" -- "$cur")) ;;
        disconnect) COMPREPLY=($(compgen -W "--all $(_tablet_profiles)" -- "$cur")) ;;
        need) COMPREPLY=($(compgen -W "--reason --timeout -- $(_tablet_profiles)" -- "$cur")) ;;
        toggle | show) COMPREPLY=($(compgen -W "--center" -- "$cur")) ;;
        status) COMPREPLY=($(compgen -W "--json" -- "$cur")) ;;
        history) COMPREPLY=($(compgen -W "--count" -- "$cur")) ;;
        doctor) COMPREPLY=($(compgen -W "--repair" -- "$cur")) ;;
    esac
}

complete -F _tablet tablet
