# alacritree keeps a WSL keeper process while a distro core runs terminals

WSL stops an idle distro shortly after its last `wsl.exe` client exits, and a systemd service or a `setsid` process does not count as activity, so a distro's core and its terminals die soon after alacritree ends. While a distro's core has running terminals, alacritree keeps one detached, hidden `wsl.exe -d <distro>` keeper for it, and the keeper exits when that core has none. A process that stays after its app ends surprises a reader, but without it the `herdr_core` session backend gives no persistence on WSL.

The resident connection starts the keeper each time its core goes from no running terminal to one, and a quit starts one more. A lock per core turns away every keeper after the first. The keeper starts before any quit, so a panic, a `taskkill` or a closed console also leaves it running.

## Considered Options

- Start the keeper only on quit. A panic or a killed process then skips the quit path, and WSL stops the distro after its idle timeout, although the terminals were kept for the next launch.
- Ask the user to set `instanceIdleTimeout=-1` in `.wslconfig`. That setting is global and the user's choice, so `doctor` only mentions it.
- Accept the loss. WSL sessions then survive only a relaunch inside the idle timeout.
