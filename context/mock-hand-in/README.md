# SwapBytes - P2P Chat and File Sharing Application

A peer-to-peer application built with libp2p that enables chat messaging and file sharing between peers.

## Prerequisites

- Rust and Cargo installed
- Network connectivity

## Quick Start

1. Clone the repository
2. Navigate to the project directory
3. Run the application:

   ```bash
   cargo run
   ```

## Usage

### Local Network Discovery

For local network discovery, simply run:

```bash
cargo run
```

### Cross-Network Connection

To connect across different networks:

1. Start a bootstrap node:

   ```bash
   cargo run
   ```

2. Note the listening address (e.g., `/ip4/127.0.0.1/tcp/56730`)
(NOTE: To copy from TUI interface hold CTRL and select the address)

3. Connect other peers using the bootstrap address:

   ```bash
   cargo run -- --bootstrap /ip4/127.0.0.1/tcp/56730
   ```

4. Or simply run:

   ```bash
   cargo run
   ```

   for automatic local network discovery

### Available Commands

- `/chat <message>` - Send a message to all peers
- `/dm <nickname> <message>` - Send a direct message
- `/getfile <nickname> <file_name> <local_path>` - Request a file
- `/list` - List all known peers
- `/help` - Display available commands

### UI Controls

- Arrow keys: Up/Down for vertical scrolling, Left/Right for horizontal scrolling
- Mouse: Up/Down for vertical scrolling
- Shift + Mouse: Up/Down for horizontal scrolling
- Press 'q' to quit (NOTE: works only on empty input)

## Example Usage

```bash
# Terminal 1 (Bootstrap Node)
cargo run

# Terminal 2 (Peer 1)
cargo run -- --bootstrap /ip4/127.0.0.1/tcp/56730

# Terminal 3 (Peer 2)
cargo run -- --bootstrap /ip4/127.0.0.1/tcp/56730

# Terminal 4 (Peer 3)
cargo run
```

### Command Examples

```bash
# Send a broadcast message to all peers
/chat Hello from Alice!

# Send a direct message to a specific peer
/dm Bob Hey Bob, can you send me that file?

# Request a file from a peer
/getfile Bob notes.txt ./bobs_notes.txt

# List all known peers
/list

# Display help
/help
```

## Notes

- Local discovery works automatically when running without bootstrap address
- File sharing requires the requested file to exist on the target peer's system
- Allow time for peer discovery may take a few seconds for late joiners to be discovered and synced up.
- File storage locations:
  - When using `./<filename>` as the path, files will be both read from and saved to the root directory where `cargo run` was executed
