use clap::Parser;
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity},
    kad::{self, Mode, store::MemoryStore},
    mdns,
    noise,
    request_response::{self, ProtocolSupport},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp,
    yamux,
    PeerId,
    Multiaddr,
    StreamProtocol,
    identity::Keypair,
    futures::StreamExt,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::time::Duration;
use tokio::io::{self, AsyncBufReadExt};
use tokio::select;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::time::interval;
use tokio::sync::mpsc;
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
    widgets::{Paragraph, Block, Borders, List, ListItem},
    layout::{Layout, Direction, Constraint},
    style::{Style, Color},
    text::{Text, Line, Span},
};
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use chrono::Local;

// ------------------------------------------------------------------------------------------------
// CLI Configuration
// ------------------------------------------------------------------------------------------------
// Parses optional bootstrap address for peer discovery
#[derive(Parser, Debug)]
struct Cli {
    #[arg(long)]
    bootstrap: Option<Multiaddr>,
}

// ------------------------------------------------------------------------------------------------
// Request and Response Types
// ------------------------------------------------------------------------------------------------
// Used for direct messaging and file swapping
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum SwapBytesRequest {
    DirectMessage(String),
    FileRequest(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum SwapBytesResponse {
    Ack,
    FileData(Vec<u8>),
}

impl fmt::Display for SwapBytesResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SwapBytesResponse::Ack => write!(f, "Ack"),
            SwapBytesResponse::FileData(_) => write!(f, "FileData"),
        }
    }
}

impl Error for SwapBytesResponse {}

// ------------------------------------------------------------------------------------------------
// Network Behaviours
// ------------------------------------------------------------------------------------------------
// Combines mdns, Kademlia, Gossipsub, and RequestResponse
#[derive(NetworkBehaviour)]
struct SwapBytesBehaviour {
    mdns: mdns::tokio::Behaviour, // For local peer discovery
    kademlia: kad::Behaviour<MemoryStore>, // For DHT-based peer discovery
    gossipsub: gossipsub::Behaviour, // For pubsub chat room
    request_response: request_response::cbor::Behaviour<SwapBytesRequest, SwapBytesResponse>, // For direct messages and file requests
}

// ------------------------------------------------------------------------------------------------
// UI Structures
// ------------------------------------------------------------------------------------------------
// Manages chat messages for display
struct ChatMessage {
    timestamp: String,
    content: String,
}

// Holds UI state for Ratatui interface
struct TuiState {
    messages: Vec<ChatMessage>, // List of chat messages
    input: String, // Current user input buffer
    scroll: u16, // Vertical scroll position
    h_scroll: u16, // Horizontal scroll position
    auto_scroll: bool, // Flag for auto-scrolling to the bottom
}

// ------------------------------------------------------------------------------------------------
// Core Application Structure
// ------------------------------------------------------------------------------------------------
// Manages swarm and application state
struct SwapBytesNode {
    swarm: libp2p::Swarm<SwapBytesBehaviour>,
    nickname: String, // user defined nickname
    peer_nicknames: HashMap<PeerId, String>, // peer nickname mapping
    pending_file_requests: HashMap<libp2p::request_response::OutboundRequestId, String>, // file request
    nickname_announced: bool, // Flag nickname announced
    bootstrap_message: Option<String>, // bootstrap status
}

// ------------------------------------------------------------------------------------------------
// SwapBytesNode
// ------------------------------------------------------------------------------------------------
impl SwapBytesNode {
    // ------------------------------------------------------------------------------------------------
    // New Function
    // ------------------------------------------------------------------------------------------------
    // inits a new SwapBytes node with peer discovery and network setup. Called from the main function.
    async fn new(nickname: String, bootstrap: Option<Multiaddr>) -> Result<(Self, String), Box<dyn Error>> {
        let keypair = Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        // Set up the swarm with behaviors for peer discovery, chat, and file swapping
        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key: &Keypair| {
                let mdns = mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?;
                let mut kademlia = kad::Behaviour::new(
                    key.public().to_peer_id(),
                    MemoryStore::new(key.public().to_peer_id()),
                );
                kademlia.set_mode(Some(Mode::Server)); // Enable Kademlia DHT
                let gossipsub = gossipsub::Behaviour::new(
                    MessageAuthenticity::Signed(key.clone()),
                    gossipsub::Config::default(),
                )?;
                let request_response = request_response::cbor::Behaviour::new(
                    [
                        (StreamProtocol::new("/swapbytes/dm/1.0.0"), ProtocolSupport::Full),
                        (StreamProtocol::new("/swapbytes/file/1.0.0"), ProtocolSupport::Full),
                    ],
                    request_response::Config::default(),
                );
                Ok(SwapBytesBehaviour {
                    mdns,
                    kademlia,
                    gossipsub,
                    request_response,
                })
            })?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        // listen on random port
        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

        // If a bootstrap address is provided, dial it and add to Kademlia (Peer discovery mechanism)
        if let Some(bootstrap_addr) = bootstrap {
            swarm.dial(bootstrap_addr.clone())?;
            swarm.behaviour_mut().kademlia.add_address(&peer_id, bootstrap_addr);
        }

        // Subscribe to join the gossipsub topic for chat (Pubsub chat room)
        let topic = IdentTopic::new("/swapbytes/chat/1.0.0");
        swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

        // bootstrap Kademlia DHT if possible, show error if not
        let mut bootstrap_message = String::new();
        if let Err(e) = swarm.behaviour_mut().kademlia.bootstrap() {
            bootstrap_message = format!("[SYSTEM]: Kademlia bootstrap failed: {}. Relying on mDNS.", e);
        }

        let node = SwapBytesNode {
            swarm,
            nickname,
            peer_nicknames: HashMap::new(),
            pending_file_requests: HashMap::new(),
            nickname_announced: false,
            bootstrap_message: Some(bootstrap_message.clone()),
        };
        Ok((node, bootstrap_message))
    }

    // ------------------------------------------------------------------------------------------------
    // Run Function
    // ------------------------------------------------------------------------------------------------
    // runs the main application loop with core logic. Handles TUI creation, calls input and event handling functions. Called from the main function.
    async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        let (tx, mut rx) = mpsc::channel::<ChatMessage>(100);
        let mut state = TuiState {
            messages: vec![],
            input: String::new(),
            scroll: 0,
            h_scroll: 0,
            auto_scroll: true,
        };

        // show initial help messages to user to guide interaction
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: Available Commands:".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /chat <message> - Send a message to all peers".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /dm <nickname> <message> - Send a direct message".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /getfile <nickname> <file_name> <local_path> - Request a file".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /list - List all known peers".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /help - Display this help message".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: Scrolling Controls:".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - Arrow keys: Up/Down for vertical, Left/Right for horizontal".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - Mouse wheel: Up/Down for vertical scrolling".to_string() }).await?;
        tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - Shift + Mouse wheel: Up/Down for horizontal scrolling".to_string() }).await?;
        tx.send(ChatMessage { timestamp, content: "[SYSTEM]: - Press 'q' to quit".to_string() }).await?;

        if let Some(bootstrap_message) = self.bootstrap_message.take() {
            tx.send(ChatMessage {
                timestamp: Local::now().format("%H:%M:%S").to_string(),
                content: bootstrap_message,
            }).await?;
        }

        // Set up event handling listeners for user inputs
        let (tx_event, mut rx_event) = mpsc::channel(100);
        let event_tx = tx_event.clone();
        tokio::spawn(async move {
            loop {
                if event::poll(Duration::from_millis(100)).unwrap() {
                    if let Ok(event) = event::read() {
                        let _ = event_tx.send(event).await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        // init the TUI terminal interface (Advanced UI)
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let mut announcement_interval = interval(Duration::from_secs(10));

        // Main event loop handling UI input, network events, and periodic tasks
        loop {
            select! {
                event = rx_event.recv() => {
                    if let Some(event) = event {
                        match event {
                            // handle user keyboard inputs
                            Event::Key(key_event) => {
                                match key_event.code {
                                    // exit application
                                    KeyCode::Char('q') => {
                                        if state.input.is_empty() {
                                            break; // exit application
                                        } else {
                                            state.input.push('q');
                                        }
                                    },
                                    // append character to input
                                    KeyCode::Char(c) => state.input.push(c),
                                    // remove character from input
                                    KeyCode::Backspace => { state.input.pop(); },
                                    // process user commands using the handle_input function
                                    KeyCode::Enter => {
                                        self.handle_input(&state.input, &tx).await?;
                                        state.input.clear();
                                    },
                                    // arrow key scroll up
                                    KeyCode::Up => {
                                        state.scroll = state.scroll.saturating_sub(1);
                                        state.auto_scroll = false;
                                    },
                                    // arrow key scroll down
                                    KeyCode::Down => {
                                        state.scroll = state.scroll.saturating_add(1);
                                    },
                                    // arrow key scroll left
                                    KeyCode::Left => {
                                        state.h_scroll = state.h_scroll.saturating_sub(1);
                                    },
                                    // arrow key scroll right
                                    KeyCode::Right => {
                                        state.h_scroll = state.h_scroll.saturating_add(1);
                                    },
                                    _ => {},
                                }
                            },

                            // handle user input mouse events
                            Event::Mouse(mouse_event) => {
                                let modifiers = mouse_event.modifiers;
                                match mouse_event.kind {
                                    // mouse scroll up events 
                                    crossterm::event::MouseEventKind::ScrollUp => {
                                        // horizontal scrolling with holding shift and scroll up to go left
                                        if modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
                                            state.h_scroll = state.h_scroll.saturating_sub(1);
                                        } else { // vertical scrolling down
                                            state.scroll = state.scroll.saturating_sub(1);
                                            // disable auto scroll when viewing past messages 
                                            // prevents scrolling to the bottom as new messages arrive
                                            state.auto_scroll = false;
                                        }
                                    },
                                    // mouse scroll down events
                                    crossterm::event::MouseEventKind::ScrollDown => {
                                        // horizontal scrolling with holding shift and scroll down to go right
                                        if modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
                                            state.h_scroll = state.h_scroll.saturating_add(1);
                                        } else { // vertical scrolling up
                                            state.scroll = state.scroll.saturating_add(1);
                                        }
                                    },
                                    _ => {},
                                }
                            },
                            _ => {},
                        }
                    }
                }

                // handle network events via the handle_event function
                swarm_event = self.swarm.select_next_some() => {
                    self.handle_event(swarm_event, &tx).await?;
                }
                
                // handle new messages
                msg = rx.recv() => {
                    if let Some(msg) = msg {
                        state.messages.push(msg); // Add new message to UI
                        if state.auto_scroll {
                            state.scroll = u16::MAX; // Auto-scroll to bottom to show new messages
                        }
                    }
                }
                
                // handle nickname announcement
                _ = announcement_interval.tick() => {
                    match self.announce_nickname().await {
                        Ok(_) => {
                            if !self.nickname_announced {
                                tx.send(ChatMessage {
                                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                                    content: format!("[SYSTEM]: Nickname '{}' announced.", self.nickname),
                                }).await?;
                                self.nickname_announced = true;
                            }
                        }
                        Err(e) => {
                            tx.send(ChatMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: format!("[SYSTEM]: Error announcing nickname: {}", e),
                            }).await?;
                        }
                    }
                }
            }

            // TUI drawing and rendering UI elements
            terminal.draw(|f| {
                // creates the horizontal layout for the UI
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(20), // 20% of the screen width for the peer list
                        Constraint::Percentage(80), // 80% of the screen width for the chat
                    ])
                    .split(f.area());

                // creates the vertical layout for the chat
                let main_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(0), // Chat area, flexible, fills remaining space
                        Constraint::Length(3), // Input area 3 row height
                        Constraint::Length(1), // Status bar 1 row height
                    ])
                    .split(chunks[1]);

                // Render peer list sidebar
                let peer_list: Vec<ListItem> = self.peer_nicknames.iter()
                    .filter(|(id, _)| *id != self.swarm.local_peer_id())
                    .map(|(id, nick)| ListItem::new(format!("{}: {}", nick, id.to_string().chars().take(8).collect::<String>())))
                    .collect();
                let peer_list_widget = List::new(peer_list)
                    .block(Block::default().title("Peers").borders(Borders::ALL))
                    .style(Style::default().fg(Color::White));
                f.render_widget(peer_list_widget, chunks[0]);

                // style and format the chat messages by type
                let messages: Vec<Line> = state.messages.iter().map(|msg| {
                    let (prefix, content, color) = if msg.content.starts_with("[SYSTEM]") {
                        ("[SYSTEM]", &msg.content[8..], Color::Gray)
                    } else if msg.content.starts_with("[DM from") {
                        ("[DM from", &msg.content[8..], Color::Yellow)
                    } else if msg.content.starts_with("[DM to") {
                        ("[DM to", &msg.content[6..], Color::Yellow)
                    } else {
                        ("", msg.content.as_str(), Color::White)
                    };
                    Line::from(vec![
                        Span::styled(format!("[{}] ", msg.timestamp), Style::default().fg(Color::Gray)),
                        Span::styled(prefix, Style::default().fg(Color::Cyan)),
                        Span::styled(content, Style::default().fg(color)),
                    ])
                }).collect();

                // prep and turn messages into TUI text for scroll calculations
                let num_lines = messages.len() as u16;
                let message_text = Text::from(messages);

                // ensure scroll positions are within bounds, vertically
                let inner_height = main_chunks[0].height.saturating_sub(2); // Account for borders
                let max_scroll = num_lines.saturating_sub(inner_height);
                state.scroll = state.scroll.min(max_scroll);
                state.auto_scroll = state.scroll == max_scroll;

                // render the chat messages in the chat area
                let message_block = Paragraph::new(message_text)
                    .block(Block::default().title("SwapBytes Chat").borders(Borders::ALL))
                    .scroll((state.scroll, state.h_scroll));
                f.render_widget(message_block, main_chunks[0]);

                // Render input area for user commands
                let input_block = Paragraph::new(format!("> {}", state.input))
                    .block(Block::default().title("Input").borders(Borders::ALL))
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(input_block, main_chunks[1]);

                // Render status bar showing connection status
                let status_text = if self.swarm.connected_peers().count() > 0 {
                    format!("Connected (Peers: {})", self.swarm.connected_peers().count())
                } else {
                    "Disconnected".to_string()
                };
                let status_bar = Paragraph::new(status_text)
                    .block(Block::default().borders(Borders::NONE))
                    .style(Style::default().fg(Color::Green));
                f.render_widget(status_bar, main_chunks[2]);
            })?;
        }

        // Clean up terminal on exit
        execute!(std::io::stdout(), LeaveAlternateScreen, crossterm::event::DisableMouseCapture)?;
        disable_raw_mode()?;
        terminal.show_cursor()?;
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------
    // Announce Nickname Function
    // ------------------------------------------------------------------------------------------------
    // broadcast the users nickname to the network via Gossipsub to chat. Called from the run function.
    async fn announce_nickname(&mut self) -> Result<(), Box<dyn Error>> {
        let topic = IdentTopic::new("/swapbytes/chat/1.0.0");
        let message = format!("JOIN {}", self.nickname);
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, message.as_bytes())?;
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------
    // Handle Input Function
    // ------------------------------------------------------------------------------------------------
    // handles user input commands. Called from the run function.
    async fn handle_input(&mut self, line: &str, tx: &mpsc::Sender<ChatMessage>) -> Result<(), Box<dyn Error>> {
        let parts: Vec<&str> = line.split_whitespace().collect();

        if parts.is_empty() {
            return Ok(());
        }

        // capture current timestamp
        let timestamp = Local::now().format("%H:%M:%S").to_string();

        match parts[0] {
            // display available commands
            "/help" => {
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: Available Commands:".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /chat <message> - Send a message to all peers".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /dm <nickname> <message> - Send a direct message".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /getfile <nickname> <file_name> <local_path> - Request a file".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /list - List all known peers".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - /help - Display this help message".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: Scrolling Controls:".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - Arrow keys: Up/Down for vertical, Left/Right for horizontal".to_string() }).await?;
                tx.send(ChatMessage { timestamp: timestamp.clone(), content: "[SYSTEM]: - Mouse wheel: Up/Down for vertical scrolling".to_string() }).await?;
                tx.send(ChatMessage { timestamp, content: "[SYSTEM]: - Shift + Mouse wheel: Up/Down for horizontal scrolling".to_string() }).await?;
            }

            // broadcast message to all peers in the pubsub chat room
            "/chat" => {
                if parts.len() > 1 {
                    let message = parts[1..].join(" ");
                    let topic = IdentTopic::new("/swapbytes/chat/1.0.0");
                    match self.swarm.behaviour_mut().gossipsub.publish(topic, message.as_bytes()) {
                        Ok(_) => {
                            tx.send(ChatMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: format!("[{}]: {}", self.nickname, message),
                            }).await?;
                        }
                        Err(gossipsub::PublishError::InsufficientPeers) => {
                            tx.send(ChatMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: "[SYSTEM]: No peers connected yet. Please wait.".to_string(),
                            }).await?;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
            }

            // send direct message to a specific peer with the request response
            "/dm" => {
                if parts.len() > 2 {
                    let target_nickname = parts[1];
                    let message = parts[2..].join(" ");
                    if let Some(&target_peer) = self.peer_nicknames.iter().find_map(|(id, nick)| {
                        if nick == target_nickname { Some(id) } else { None }
                    }) {
                        self.swarm
                            .behaviour_mut()
                            .request_response
                            // Send direct message
                            .send_request(&target_peer, SwapBytesRequest::DirectMessage(message.clone()));
                        tx.send(ChatMessage {
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            content: format!("[DM to {}]: {}", target_nickname, message),
                        }).await?;
                    } else {
                        tx.send(ChatMessage {
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            content: format!("[SYSTEM]: Nickname '{}' not found.", target_nickname),
                        }).await?;
                    }
                }
            }

            // request a file from a specific peer with the request response
            "/getfile" => {
                if parts.len() == 4 {
                    let target_nickname = parts[1];
                    let file_name = parts[2];
                    let local_path = parts[3];
                    if let Some(&target_peer) = self.peer_nicknames.iter().find_map(|(id, nick)| {
                        if nick == target_nickname { Some(id) } else { None }
                    }) {
                        let request_id = self.swarm
                            .behaviour_mut()
                            .request_response
                             // Request file from peer
                            .send_request(&target_peer, SwapBytesRequest::FileRequest(file_name.to_string()));
                        self.pending_file_requests.insert(request_id, local_path.to_string());
                        tx.send(ChatMessage {
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            content: format!("[SYSTEM]: Requesting '{}' from {}", file_name, target_nickname),
                        }).await?;
                    } else {
                        tx.send(ChatMessage {
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            content: format!("[SYSTEM]: Nickname '{}' not found.", target_nickname),
                        }).await?;
                    }
                } else {
                    tx.send(ChatMessage {
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        content: "[SYSTEM]: Usage: /getfile <nickname> <file_name> <local_path>".to_string(),
                    }).await?;
                }
            }

            // list all known peers using nicknames excluding self
            "/list" => {
                tx.send(ChatMessage {
                    timestamp: timestamp.clone(),
                    content: "[SYSTEM]: Known peers:".to_string(),
                }).await?;
                let local_peer_id = *self.swarm.local_peer_id();
                for (peer_id, nick) in &self.peer_nicknames {
                    if *peer_id != local_peer_id {
                        tx.send(ChatMessage {
                            timestamp: timestamp.clone(),
                            content: format!("- {}: {}", nick, peer_id),
                        }).await?;
                    }
                }
            }
            // handle unknown commands
            _ => tx.send(ChatMessage {
                timestamp,
                content: format!("[SYSTEM]: Unknown command: {}", parts[0]),
            }).await?,
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------
    // Handle Event Function
    // ------------------------------------------------------------------------------------------------
    // handles network events from libp2p swarm like peer discovery, messages, and file transfers. Called from the run function.
    async fn handle_event(&mut self, event: SwarmEvent<SwapBytesBehaviourEvent>, tx: &mpsc::Sender<ChatMessage>) -> Result<(), Box<dyn Error>> {
        // match the event type and handle accordingly
        match event {
            // inform user of address the node is listening on
            SwarmEvent::NewListenAddr { address, .. } => {
                tx.send(ChatMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    content: format!("[SYSTEM]: Listening on {}", address),
                }).await?;
            }

            // handle peer discovery events via mDNS initially
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                for (peer_id, multiaddr) in list {
                    tx.send(ChatMessage {
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        content: format!("[SYSTEM]: Discovered peer: {} at {}", peer_id, multiaddr),
                    }).await?;
                    // add discovered peer to Kademlia for routing
                    self.swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr.clone());
                    // add peer to gossipsub for pubsub messaging
                    self.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                }
            }

            // handle peer successful connection events
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                tx.send(ChatMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    content: format!("[SYSTEM]: Connected to peer: {}", peer_id),
                }).await?;
            }

            // handle pubsub messages
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                let msg = String::from_utf8_lossy(&message.data);
                if let Some(source) = message.source {
                    // handle nickname announcement to chat
                    if msg.starts_with("JOIN ") {
                        let nickname = msg.trim_start_matches("JOIN ").to_string();
                        if self.peer_nicknames.contains_key(&source) {
                            let existing_nick = self.peer_nicknames.get(&source).unwrap();
                            // update nickname if it has changed
                            if existing_nick != &nickname {
                                self.peer_nicknames.insert(source, nickname.clone());
                                tx.send(ChatMessage {
                                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                                    content: format!("[SYSTEM]: Peer {} changed nickname to {}", source, nickname),
                                }).await?;
                            }
                        } else {
                             // Store new peer nickname
                            self.peer_nicknames.insert(source, nickname.clone());
                            tx.send(ChatMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: format!("[SYSTEM]: {} ({}) joined the chat.", nickname, source),
                            }).await?;
                        }
                    // handle pubsub messages from peers in chat
                    } else {
                        // display message with source nickname
                        if let Some(n) = self.peer_nicknames.get(&source) {
                            tx.send(ChatMessage { // Display chat room message (Pubsub chat room)
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: format!("[{}]: {}", n, msg),
                            }).await?;
                        // display message without source
                        } else {
                            tx.send(ChatMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: format!("[{}]: {}", source, msg),
                            }).await?;
                        }
                    }
                } else {
                    // display message without source
                    tx.send(ChatMessage {
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        content: format!("[SYSTEM]: Received message without source: {}", msg),
                    }).await?;
                }
            }

            // handle request response messages
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::RequestResponse(
                request_response::Event::Message { peer, message, .. }
            )) => match message {

                // handle request
                request_response::Message::Request { request, channel, .. } => {
                    match request {
                        // handle direct message request
                        SwapBytesRequest::DirectMessage(msg) => {
                            let peer_str = peer.to_string();
                            let sender = self.peer_nicknames.get(&peer).unwrap_or(&peer_str);
                            tx.send(ChatMessage { // Receive direct message
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: format!("[DM from {}]: {}", sender, msg),
                            }).await?;
                            self.swarm
                                .behaviour_mut()
                                .request_response
                                .send_response(channel, SwapBytesResponse::Ack)?;
                        }

                        // handle file request
                        SwapBytesRequest::FileRequest(file_name) => {
                            let mut file_data = Vec::new();
                            if let Ok(mut file) = File::open(&file_name).await {
                                file.read_to_end(&mut file_data).await?;
                            }
                            self.swarm
                                .behaviour_mut()
                                .request_response
                                // Send requested file data
                                .send_response(channel, SwapBytesResponse::FileData(file_data))?; 
                        }
                    }
                }

                // handle response
                request_response::Message::Response { request_id, response } => {
                    match response {
                        // handle direct message response
                        SwapBytesResponse::Ack => {
                            tx.send(ChatMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                content: "[SYSTEM]: Direct message delivered.".to_string(),
                            }).await?;
                        }

                        // handle file response
                        SwapBytesResponse::FileData(data) => {
                            if let Some(local_path) = self.pending_file_requests.remove(&request_id) {
                                // save file to local path
                                if !data.is_empty() {
                                    tokio::fs::write(&local_path, &data).await?;
                                    tx.send(ChatMessage { // Save received file
                                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                                        content: format!("[SYSTEM]: File saved to {}", local_path),
                                    }).await?;
                                // display error if file is not found or empty
                                } else {
                                    tx.send(ChatMessage {
                                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                                        content: "[SYSTEM]: File not found or empty.".to_string(),
                                    }).await?;
                                }
                            }
                        }
                    }
                }
            },
            // handle unknown events
            _ => {}
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------
// Main function 
// ------------------------------------------------------------------------------------------------
// CLI, get nickname, init and run the SwapBytesNode
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // parse CLI arguments, get bootstrap address if provided
    let cli = Cli::parse();

    // set buffer reader for user input
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // prompt user for their nickname
    println!("Enter your nickname:");

    // valid name, strip whitespace, uses fall back if user just presses enter
    let nickname = match stdin.next_line().await {
        Ok(Some(line)) => line.trim().to_string(),
        _ => "Anonymous".to_string(),
    };

    // init SwapBytesNode with new function with nickname and bootstrap address
    let (mut node, _) = SwapBytesNode::new(nickname, cli.bootstrap).await?;

    // call SwapBytesNode run function to start application
    node.run().await?;

    Ok(())
}