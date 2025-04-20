# SwapBytes

## Technical Explanation of how the application works

## Application Start

The application starts in `main.rs` where:

```rust
fn main() 
```

is called and performs the following:

1. **Command Line Parsing**
   - Parses command line arguments
   - Extracts optional bootstrap address if provided
   - Asks user for nickname

2. **SwapBytesNode New Function**

   - Calls `SwapBytesNode::new` function
   - User provides nickname and optional bootstrap address
   - System generates cryptographic identity
   - Creates unique PeerId for node identification
   - Initializes network stack and protocols:
     - TCP transport layer with Noise encryption
     - mDNS service
     - Kademlia DHT
     - Gossipsub
     - Request-Response
     - Sets up peer discovery mechanisms

3. **SwapBytesNode Run Function**
   - Calls `SwapBytesNode::run` function
   - Preps a initial help message to display to the user
   - Enters the main event loop
     - Handles user keyboard inputs
       - Handles user keyboard command inputs with handle_input function
     - Handles user mouse inputs
     - Handles network events via the handle_event function
       - Peer discovery
       - Chat message handling
       - Direct message handling
       - File transfer handling
     - Handles new messages (new chat messages)
     - Handles nickname announcement (new peer joins the network)
     - TUI interface is drawn and each element is rendered and updated
   - Runs a clean up when the user quits the application

## Peer Discovery Process

1. **Local Network Discovery (mDNS)**
   - Broadcasts node presence on local network
   - Listens for other nodes' announcements
   - When peer discovered:
     - Adds to Kademlia routing table
     - Establishes direct connection
     - Exchanges peer information
     - Updates UI with new peer

2. **Global Network Discovery (Kademlia)**
   - Maintains distributed routing table
   - Performs iterative peer lookups
   - Bootstrap process:
     - Connects to bootstrap node
     - Downloads initial peer list
     - Begins DHT queries
   - Updates peer list in real-time

## Message Handling System

1. **Chat Messages (Gossipsub)**
   - User types message and sends via `/chat` command
   - Message flow:
     1. Signs message with node's private key
     2. Publishes to chat topic
     3. Gossipsub protocol:
        - Propagates message through mesh network
        - Ensures message delivery to all subscribers
        - Handles message deduplication
     4. Receiving nodes:
        - Verify message signature
        - Display in chat interface
        - Update message history

2. **Direct Messages (Request-Response)**
   - User initiates with `/dm` command
   - Message flow:
     1. Looks up target peer's address
     2. Establishes direct connection
     3. Sends encrypted message
     4. Waits for acknowledgment
     5. Updates UI with delivery status

## File Transfer System

1. **File Request Process**
   - User initiates with `/getfile` command
   - System flow:
     1. Validates target peer exists
     2. Creates file request
     3. Establishes direct connection
     4. Sends request with filename
     5. Tracks request status
     6. Updates UI with progress

2. **File Transfer Protocol**
   - Request handling:
     1. Receives file request
     2. Validates file exists
     3. Reads file in chunks
     4. Sends data over secure channel
   - Response handling:
     1. Receives file data
     2. Writes to local filesystem
     3. Verifies transfer completion
     4. Updates UI with status

## User Interface System

1. **Terminal Interface (Ratatui)**
   - Layout components:
     - Peer list (20% width)
     - Chat area (80% width)
       - Includes Input area
       - Includes Status bar
   - Features:
     - Real-time message display
     - Command input
     - Vertical and Horizontal chat scrolling
     - Peer status updates

2. **Input Processing**
   - User Input Command parsing:
     - `/chat` - Broadcast message
     - `/dm` - Direct message
     - `/getfile` - File request
     - `/list` - Peer listing
     - `/help` - Command help
   - User Interaction Event handling:
     - Keyboard input
     - Mouse scrolling
     - Window resizing

## State Management

1. **Peer State**
   - Maintains:
     - PeerId to nickname mapping
     - Connection status
     - Last seen timestamp
   - Updates on:
     - Peer discovery
     - Nickname changes
     - Connection events

2. **File Transfer State**
   - Tracks:
     - Active transfers
     - Request IDs
     - Local file paths
     - Transfer status
   - Manages:
     - Concurrent transfers
     - Error recovery
     - Progress updates

## Error Handling System

1. **Error Handling**

- General errors:
  - Error handling where obvious to prevent application from crashing

## Any challenges you faced, and how you approached them

