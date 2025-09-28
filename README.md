# MQTT Shell - Agent and Controller with Full TTY Support

System to execute remote shell commands over MQTT with full support for interactive applications like `nano`, `vim`, `htop`, etc.

## 🚀 Main Features

- **Full remote shell** with PTY (pseudo-terminal)
- **Raw TTY mode** for interactive apps
- **Automatic terminal resizing**
- **Full ANSI escape sequence support**
- **Special keys** (arrows, F1-F12, Ctrl+C, etc.)
- **Real-time communication** via MQTT

## Components

- **Agent**: Runs `/bin/bash` in a PTY and handles commands via MQTT
- **Controller**: Interactive terminal with full TTY support

## MQTT Topics

- `<channel>/in`: Input sent to the shell (raw data, character by character)
- `<channel>/out`: Shell output (including ANSI sequences)
- `<channel>/resize`: Terminal resize information
- `<channel>/status`: Shell/agent status

## Prerequisites

1. **MQTT Broker**: You need a running MQTT broker (default: `localhost:1883`)

   To install and run Mosquitto:
   ```bash
   # Ubuntu/Debian
   sudo apt install mosquitto mosquitto-clients
   sudo systemctl start mosquitto

   # Or run manually
   mosquitto -p 1883
   ```

## Build

```bash
cargo build
# Or for release
cargo build --release
```

## Usage

### 1. Run the Agent

In one terminal:
```bash
cargo run --bin agent -- --channel shell
```

Agent output:
```
Starting MQTT Shell Agent with auto-reconnection and shell restart...
Using channel: 'shell' at localhost:1883
Shell started in PTY
Subscribed to MQTT topics
Connected to MQTT broker
```

### 2. Run the Controller

In another terminal:
```bash
cargo run --bin controller -- --channel shell
```

Controller output:
```
Starting MQTT Shell Controller with TTY support...
📡 Using channel: 'shell' at localhost:1883
🔍 Controller subscribed to shell/out and shell/status
Controller connected. Terminal size: 120x30
Press Ctrl+Q to exit.
You can now use editors like nano, vim, etc.
```

### 3. Use Interactive Applications

You can now run any TTY application:

```bash
nano file.txt
vim config.sh
htop
man ls
less file.log
tmux
screen
```

## Supported Special Keys

| Key            | Function                |
|----------------|------------------------|
| `Ctrl+Q`       | Exit controller        |
| `Ctrl+C`       | Interrupt (SIGINT)     |
| `Ctrl+Z`       | Suspend (SIGTSTP)      |
| `↑↓←→`         | Navigation             |
| `Home/End`     | Line start/end         |
| `Page Up/Down` | Page navigation        |
| `F1-F12`       | Function keys          |
| `Tab`          | Autocomplete           |
| `Backspace/Delete` | Delete characters  |

## Example nano session

```bash
# Terminal 1 - Agent
$ cargo run --bin agent
Starting MQTT Shell Agent with auto-reconnection and shell restart...
Shell started in PTY
Subscribed to MQTT topics
Connected to MQTT broker

# Terminal 2 - Controller
$ cargo run --bin controller
Starting MQTT Shell Controller with TTY support...
Controller connected. Terminal size: 120x30
Press Ctrl+Q to exit.
You can now use editors like nano, vim, etc.
bash-5.1$ nano test.txt
  GNU nano 6.2                    test.txt

Hello world!
This is a test of nano running remotely.

^G Help      ^O Write Out ^W Where Is  ^K Cut       ^T Execute   ^C Location
^X Exit      ^R Read File ^\ Replace   ^U Paste     ^J Justify   ^/ Go To Line
```

## Automatic Resizing

The controller automatically detects terminal window size changes and updates the remote PTY.

## Debugging

To view MQTT messages directly:

```bash
mosquitto_sub -t '#'
mosquitto_sub -t 'shell/in' | hexdump -C
mosquitto_sub -t 'shell/out'
mosquitto_sub -t 'shell/resize'
echo -e "ls\n" | mosquitto_pub -t 'shell/in' -s
```

## Architecture

```
[Controller TTY] --> MQTT --> [Agent PTY]
       ^                          |
       |                          v
   Raw Mode                  /bin/bash
   Escape Seqs               Full TTY
   Resize Events             ANSI Colors
       ^                          |
       |                          v
       +-------- MQTT <-----------+
```

### Data Flow

1. **Input**: Controller captures keystrokes in raw mode → Sends bytes via MQTT → Agent writes to PTY
2. **Output**: Shell generates output with ANSI sequences → Agent sends via MQTT → Controller displays directly
3. **Resize**: Controller detects size changes → Sends event → Agent resizes PTY
