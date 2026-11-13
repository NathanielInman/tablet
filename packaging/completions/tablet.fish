# fish completion for tablet
function __tablet_profiles
    set -l dir $TABLET_CONFIG_DIR
    test -n "$dir"; or set dir (test -n "$XDG_CONFIG_HOME"; and echo $XDG_CONFIG_HOME; or echo ~/.config)/tablet
    string match -r -g '^\s*name\s*=\s*"([^"]*)"' < $dir/config.toml 2>/dev/null
end

set -l commands connect disconnect reconnect flip ack signin check why need status history doctor toggle show hide bar daemon restart help
complete -c tablet -f
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -l version -d 'print the version'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a connect -d 'start a session (no profile: the default one)'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a disconnect -d "end a profile's session, or every session"
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a reconnect -d 'end the session and start a new one'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a flip -d 'what a click on the lamp does'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a ack -d 'dismiss a dropped or failed profile'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a signin -d 'open the waiting sign-in page again'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a check -d 'check now: probes, DNS and traffic'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a why -d 'which profile a host needs and whether it is up'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a need -d 'run a command once the profile is connected'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a status -d 'every profile and session'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a history -d 'what happened, newest first'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a doctor -d 'check openvpn3, the config and the bar'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a toggle -d 'open or close the drawer from the clicked icon'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a show -d 'open the drawer'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a hide -d 'close the drawer'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a bar -d 'the waybar module (JSON lines)'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a daemon -d 'run the daemon in the foreground'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a restart -d 'restart the daemon and the drawer'
complete -c tablet -n "not __fish_seen_subcommand_from $commands" -a help -d 'show the usage'
complete -c tablet -n "__fish_seen_subcommand_from connect reconnect ack signin check need" -a "(__tablet_profiles)"
complete -c tablet -n "__fish_seen_subcommand_from disconnect" -a "(__tablet_profiles)"
complete -c tablet -n "__fish_seen_subcommand_from disconnect" -l all -d 'every session'
complete -c tablet -n "__fish_seen_subcommand_from need" -l reason -r -d 'why the agent needs it'
complete -c tablet -n "__fish_seen_subcommand_from need" -l timeout -r -d 'seconds to wait for you'
complete -c tablet -n "__fish_seen_subcommand_from toggle show" -l center -d 'open centered'
complete -c tablet -n "__fish_seen_subcommand_from status" -l json -d 'print the snapshot as JSON'
complete -c tablet -n "__fish_seen_subcommand_from history" -l count -r -d 'how many entries'
complete -c tablet -n "__fish_seen_subcommand_from doctor" -l repair -d 'run doctor.repair'
