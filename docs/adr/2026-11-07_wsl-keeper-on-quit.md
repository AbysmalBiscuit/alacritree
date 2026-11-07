# alacritree leaves a WSL keeper process when it quits

WSL stops an idle distro shortly after its last `wsl.exe` client exits, and a systemd service or a `setsid` process does not count as activity, so a distro's core and its terminals die soon after alacritree quits. On quit, alacritree leaves one detached, hidden `wsl.exe -d <distro>` keeper for each distro whose core has live terminals, and the keeper exits when that core has none. A process that stays after its app quits surprises a reader, but without it the `herdr_core` session backend gives no persistence on WSL.

## Considered Options

- Ask the user to set `instanceIdleTimeout=-1` in `.wslconfig`. That setting is global and the user's choice, so `doctor` only mentions it.
- Accept the loss. WSL sessions then survive only a relaunch inside the idle timeout.
