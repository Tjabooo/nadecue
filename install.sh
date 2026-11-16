#!/bin/sh
# Build nadecue, install it for this user, put the card's eww config in place and start the daemon.
set -eu
cd "$(dirname "$0")"
for tool in cargo eww grim; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "nadecue needs $tool (cargo to build; eww draws the card; grim takes the pictures)" >&2
        exit 1
    fi
done
if [ -n "${NADECUE_BINARY:-}" ]; then
    binary="$NADECUE_BINARY"
else
    cargo build --release --locked
    binary=target/release/nadecue
fi
config="${XDG_CONFIG_HOME:-$HOME/.config}/nadecue"
install -Dm755 "$binary" "$HOME/.local/bin/nadecue"
install -Dm644 eww/eww.yuck "$config/eww/eww.yuck"
install -Dm644 eww/eww.scss "$config/eww/eww.scss"
[ -e "$config/config.toml" ] || install -Dm644 config.example.toml "$config/config.toml"
install -Dm644 nadecue.service "$HOME/.config/systemd/user/nadecue.service"
systemctl --user daemon-reload
systemctl --user enable nadecue.service
systemctl --user restart nadecue.service
echo "nadecue is running. Restart CS2 once so it picks up the game state config, then bind keys to"
echo "'nadecue save' and 'nadecue next' (see README)."
