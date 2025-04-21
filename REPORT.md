# SwapBytes

## Technical Explanation of how the application works

### Application Start

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
       - Handles user keyboard command inputs with `handle_input` function
     - Handles user mouse inputs
     - Handles network events via the `handle_event` function
       - Peer discovery
       - Chat message handling
       - Direct message handling
       - File transfer handling
     - Handles new messages (new chat messages)
     - Handles nickname announcement via `announce_nickname` function (new peer joins the network)
     - TUI interface is drawn and each element is rendered and updated
   - Runs a clean up when the user quits the application

### Peer Discovery Process

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

### Message Handling System

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

### File Transfer System

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

### User Interface System

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

### State Management

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

### Error Handling System

1. **Error Handling**

- General errors:
  - Error handling where obvious to prevent application from crashing

## Any challenges you faced, and how you approached them

### Challenge: Handling Late Joiners in the Network

One challenge faced was when a third peer joined the network after two peers were already connected and could not find the other existing peers through the `/list` command.

**Problem Flow:**

- First two peers (Peer A and Peer B) connect and can see each other.
- Third peer (Peer C) joins using bootstrap flag and address.
- Third peer (Peer C) can connect to bootstrap node but can't see the other peers.
- `/list` command shows empty peer list for the third peer.

### Handling: Handling Late Joiners in the Network

**Problem Solution:**

1. **Bootstrap Connection**  
   Peer C connects to Peer A (bootstrap node) and seeds its routing table:  

   ```rust
   swarm.dial(bootstrap_addr.clone())?;
   swarm.behaviour_mut().kademlia.add_address(&peer_id, bootstrap_addr);
   ```

2. **Kademlia DHT Discovery**  
   Peer C bootstraps its DHT to find other peers (e.g., Peer B):  

   ```rust
   swarm.behaviour_mut().kademlia.bootstrap()?;
   ```  

   Continuous lookups gradually populate its routing table.

3. **Gossipsub Announcements**  
   All peers subscribe to a chat topic and announce their presence:  

   ```rust
   let topic = IdentTopic::new("/swapbytes/chat/1.0.0");
   swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
   let message = format!("JOIN {}", self.nickname);
   swarm.behaviour_mut().gossipsub.publish(topic, message.as_bytes())?;
   ```

4. **Updating Peer List**  
   Peer C receives "JOIN" messages (e.g., from Peer B via Peer A) and updates its `peer_nicknames`:  

   ```rust
   if msg.starts_with("JOIN ") {
       let nickname = msg.trim_start_matches("JOIN ").to_string();
       self.peer_nicknames.insert(source, nickname);
   }
   ```  

   The `/list` command then displays these peers.

5. **Eventual Integration**  
   Over time, Kademlia connects Peer C to Peer B directly, and Gossipsub ensures Peer C learns about Peer B quickly via propagated messages.

### Challenge: TUI Text Display and Scrolling Limitations

Another challenge was implementing the chat message display in the TUI interface. The initial implementation used text wrapping to fit messages within the terminal width, but this caused messages to be cut off and inaccessible until newer messages pushed older messages up. This had the effect that messages were 'hidden' behind the input area and couldn't be scrolled to view.

**Problem Flow:**

- Implemented text wrapping using textwrap crate.
- Once messages approached the bottom of the terminal, they were cut off and inaccessible until newer messages pushed them up.
- Could not scroll vertically further down to view the 'hidden' messages.

### Handling: TUI Text Display and Scrolling Limitations

**Problem Solution:**
The issue was resolved through a multi-step approach:

1. Removed text wrapping in favour of horizontal scrolling:

   ```rust
   let message_block = Paragraph::new(message_text)
       .block(Block::default().title("SwapBytes Chat").borders(Borders::ALL))
       .scroll((state.scroll, state.h_scroll));
   f.render_widget(message_block, main_chunks[0]);
   ```

   - Messages now display as single lines and are cut off at the end of the terminal if they are longer than the terminal width.
   - Scroll calculations now accurately track message positions as they relied on each message being a single line.

2. However, this produced the problem of horizontal scrolling not actually working. This being left right trackpad scrolling not working.

   ```rust
   if modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
       // Shift + scroll up = scroll left
       if mouse_event.kind == crossterm::event::MouseEventKind::ScrollUp {
           state.h_scroll = state.h_scroll.saturating_sub(1);
       }
       // Shift + scroll down = scroll right
       else if mouse_event.kind == crossterm::event::MouseEventKind::ScrollDown {
           state.h_scroll = state.h_scroll.saturating_add(1);
       }
   }
   ```

   - Discovered that this was due to the terminal not supporting horizontal scrolling. And that different terminals supported different ways of scrolling.
   - Added support for Shift + vertical scrolling to allow horizontal scrolling and cross terminal compatibility.

### Challenge: Timestamp Synchronization in Chat Messages

Another challenge faced was getting timestamps for each message sent or received using the `chrono` crate. However, the all timestamps displayed in the chat would then periodically update to the current time.

**Problem Flow:**

- start application and sent and recived messages are displayed in chat with different timestamps.
- after a period of time all timestamps displayed are updated to the current time.

### Handling: Timestamp Synchronization in Chat Messages

**Problem Solution:**

The timestamp synchronization issue was resolved by shifting from generating timestamps during UI rendering to storing them with each message at creation time:

1. **Updated the `ChatMessage` Structure**  
   - include a `timestamp` field for each message to carry its own timestamp:
  
   ```rust
   struct ChatMessage {
       timestamp: String,
       content: String,
   }
   ```

2. **Generated Timestamp at Message Creation**  
   - When a new message is sent or received, the timestamp is generated using `chrono::Local::now()` and stored with the message.

   ```rust
   let timestamp = Local::now().format("%H:%M:%S").to_string();
   let new_message = ChatMessage {
       timestamp,
       content: format!("[{}]: {}", self.nickname, message),
   };
   state.messages.push(new_message);
   ```

   - The timestamp now reflects the actual time instead of when it's displayed.

3. **Used Stored Timestamp in Rendering**  
   - rendering logic then uses the stored `timestamp` from each `ChatMessage` instead of recalculating it.

   ```rust
   let messages: Vec<Line> = state.messages.iter().map(|msg| {
       Line::from(vec![
           Span::styled(format!("[{}] ", msg.timestamp), Style::default().fg(Color::Gray)),
           Span::styled(msg.content.as_str(), Style::default().fg(Color::White)),
       ])
   }).collect();
   ```

   - each message displays timestamp, unaffected by UI refreshes.
ß