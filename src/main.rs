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
    widgets::{Paragraph, Block, Borders},
    layout::{Layout, Direction, Constraint},
    style::{Style, Color},
    text::{Text, Line, Span},
};
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long)]
    bootstrap: Option<Multiaddr>,
}

#[derive(NetworkBehaviour)]
struct SwapBytesBehaviour {
    mdns: mdns::tokio::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    request_response: request_response::cbor::Behaviour<SwapBytesRequest, SwapBytesResponse>,
}

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

// TUI State and Interface Start
struct TuiState {
    messages: Vec<String>,
    input: String,
    scroll: usize,
}

struct SwapBytesNode {
    swarm: libp2p::Swarm<SwapBytesBehaviour>,
    nickname: String,
    peer_nicknames: HashMap<PeerId, String>,
    pending_file_requests: HashMap<libp2p::request_response::OutboundRequestId, String>,
    nickname_announced: bool,
}

impl SwapBytesNode {
    async fn new(nickname: String, bootstrap: Option<Multiaddr>) -> Result<Self, Box<dyn Error>> {
        let keypair = Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

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
                kademlia.set_mode(Some(Mode::Server));
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

        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

        if let Some(bootstrap_addr) = bootstrap {
            swarm.dial(bootstrap_addr.clone())?;
            swarm.behaviour_mut().kademlia.add_address(&peer_id, bootstrap_addr);
        }

        let topic = IdentTopic::new("/swapbytes/chat/1.0.0");
        swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

        if let Err(e) = swarm.behaviour_mut().kademlia.bootstrap() {
            println!("Kademlia bootstrap failed: {}. Relying on mDNS for peer discovery.", e);
        }

        let node = SwapBytesNode {
            swarm,
            nickname,
            peer_nicknames: HashMap::new(),
            pending_file_requests: HashMap::new(),
            nickname_announced: false,
        };
        Ok(node)
    }

    async fn announce_nickname(&mut self) -> Result<(), Box<dyn Error>> {
        let topic = IdentTopic::new("/swapbytes/chat/1.0.0");
        let message = format!("JOIN {}", self.nickname);
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, message.as_bytes())?;
        Ok(())
    }

    async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        // TUI Setup: Channels, State, Event Loop
        let (tx, mut rx) = mpsc::channel(100);
        let mut state = TuiState {
            messages: vec![],
            input: String::new(),
            scroll: 0,
        };

        let (tx_event, mut rx_event) = mpsc::channel(100);
        let event_tx = tx_event.clone();
        
        // TUI Event Handler
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

        // TUI Terminal Setup
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let mut announcement_interval = interval(Duration::from_secs(10));

        // TUI Main Loop
        loop {
            select! {
                // TUI Event Handling: Key Input, Scrolling
                event = rx_event.recv() => {
                    if let Some(Event::Key(key_event)) = event {
                        match key_event.code {
                            KeyCode::Char('q') => break, // Quit on 'q'
                            KeyCode::Char(c) => state.input.push(c),
                            KeyCode::Backspace => { state.input.pop(); },
                            KeyCode::Enter => {
                                self.handle_input(&state.input, &tx).await?;
                                state.input.clear();
                            },
                            KeyCode::Up => {
                                state.scroll = state.scroll.saturating_sub(1);
                            },
                            KeyCode::Down => {
                                state.scroll += 1;
                            },
                            _ => {},
                        }
                    }
                }
                swarm_event = self.swarm.select_next_some() => {
                    self.handle_event(swarm_event, &tx).await?;
                }
                msg = rx.recv() => {
                    if let Some(msg) = msg {
                        state.messages.push(msg);
                    }
                }
                _ = announcement_interval.tick() => {
                    match self.announce_nickname().await {
                        Ok(_) => {
                            if !self.nickname_announced {
                                tx.send(format!("[SYSTEM]: Nickname '{}' announced successfully.", self.nickname)).await?;
                                self.nickname_announced = true;
                            }
                        }
                        Err(e) => {
                            tx.send(format!("[SYSTEM]: Error announcing nickname: {}", e)).await?;
                        }
                    }
                }
            }

            // TUI Rendering: Message Display, Input Box
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)])
                    .split(f.area());

                // Message Display with Styling
                let messages: Vec<Line> = state.messages.iter().map(|m| {
                    if m.starts_with("[SYSTEM]") {
                        Line::from(Span::styled(m.clone(), Style::default().fg(Color::Gray)))
                    } else if m.starts_with("[DM from") || m.starts_with("[DM to") {
                        Line::from(Span::styled(m.clone(), Style::default().fg(Color::Yellow)))
                    } else {
                        Line::from(m.clone())
                    }
                }).collect();
                let num_lines = messages.len();
                let area_height = chunks[0].height as usize;
                let max_scroll = num_lines.saturating_sub(area_height);
                state.scroll = state.scroll.min(max_scroll);

                // Message Block with Scroll
                let message_block = Paragraph::new(Text::from(messages))
                    .block(Block::default().title("SwapBytes Chat").borders(Borders::ALL))
                    .scroll((state.scroll as u16, 0));
                f.render_widget(message_block, chunks[0]);

                // Input Box
                let input_block = Paragraph::new(format!("> {}", state.input))
                    .block(Block::default().title("Input").borders(Borders::ALL));
                f.render_widget(input_block, chunks[1]);
            })?;
        }

        // TUI Cleanup
        execute!(std::io::stdout(), LeaveAlternateScreen)?;
        disable_raw_mode()?;
        terminal.show_cursor()?;
        Ok(())
    }

    async fn handle_input(&mut self, line: &str, tx: &mpsc::Sender<String>) -> Result<(), Box<dyn Error>> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        match parts[0] {
            "/chat" => {
                if parts.len() > 1 {
                    let message = parts[1..].join(" ");
                    let topic = IdentTopic::new("/swapbytes/chat/1.0.0");
                    match self.swarm.behaviour_mut().gossipsub.publish(topic, message.as_bytes()) {
                        Ok(_) => {
                            tx.send(format!("[{}]: {}", self.nickname, message)).await?;
                        }
                        Err(gossipsub::PublishError::InsufficientPeers) => {
                            tx.send("[SYSTEM]: No peers connected yet. Please wait.".to_string()).await?;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
            }
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
                            .send_request(&target_peer, SwapBytesRequest::DirectMessage(message.clone()));
                        tx.send(format!("[DM to {}]: {}", target_nickname, message)).await?;
                    } else {
                        tx.send(format!("[SYSTEM]: Nickname '{}' not found.", target_nickname)).await?;
                    }
                }
            }
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
                            .send_request(&target_peer, SwapBytesRequest::FileRequest(file_name.to_string()));
                        self.pending_file_requests.insert(request_id, local_path.to_string());
                        tx.send(format!("[SYSTEM]: Requesting '{}' from {}", file_name, target_nickname)).await?;
                    } else {
                        tx.send(format!("[SYSTEM]: Nickname '{}' not found.", target_nickname)).await?;
                    }
                } else {
                    tx.send("[SYSTEM]: Usage: /getfile <nickname> <file_name> <local_path>".to_string()).await?;
                }
            }
            "/list" => {
                tx.send("[SYSTEM]: Known peers:".to_string()).await?;
                let local_peer_id = *self.swarm.local_peer_id();
                for (peer_id, nick) in &self.peer_nicknames {
                    if *peer_id != local_peer_id {
                        tx.send(format!("- {}: {}", nick, peer_id)).await?;
                    }
                }
            }
            _ => tx.send(format!("[SYSTEM]: Unknown command: {}", parts[0])).await?,
        }
        Ok(())
    }

    async fn handle_event(&mut self, event: SwarmEvent<SwapBytesBehaviourEvent>, tx: &mpsc::Sender<String>) -> Result<(), Box<dyn Error>> {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                tx.send(format!("[SYSTEM]: Listening on {}", address)).await?;
            }
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                for (peer_id, multiaddr) in list {
                    tx.send(format!("[SYSTEM]: Discovered peer: {} at {}", peer_id, multiaddr)).await?;
                    self.swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr.clone());
                    self.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                tx.send(format!("[SYSTEM]: Connected to peer: {}", peer_id)).await?;
            }
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                let msg = String::from_utf8_lossy(&message.data);
                if let Some(source) = message.source {
                    if msg.starts_with("JOIN ") {
                        let nickname = msg.trim_start_matches("JOIN ").to_string();
                        if self.peer_nicknames.contains_key(&source) {
                            let existing_nick = self.peer_nicknames.get(&source).unwrap();
                            if existing_nick != &nickname {
                                self.peer_nicknames.insert(source, nickname.clone());
                                tx.send(format!("[SYSTEM]: Peer {} changed nickname to {}", source, nickname)).await?;
                            }
                        } else {
                            self.peer_nicknames.insert(source, nickname.clone());
                            tx.send(format!("[SYSTEM]: {} ({}) joined the chat.", nickname, source)).await?;
                        }
                    } else {
                        if let Some(n) = self.peer_nicknames.get(&source) {
                            tx.send(format!("[{}]: {}", n, msg)).await?;
                        } else {
                            tx.send(format!("[{}]: {}", source, msg)).await?;
                        }
                    }
                } else {
                    tx.send(format!("[SYSTEM]: Received message without source: {}", msg)).await?;
                }
            }
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::RequestResponse(
                request_response::Event::Message { peer, message, .. }
            )) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    match request {
                        SwapBytesRequest::DirectMessage(msg) => {
                            let peer_str = peer.to_string();
                            let sender = self.peer_nicknames.get(&peer).unwrap_or(&peer_str);
                            tx.send(format!("[DM from {}]: {}", sender, msg)).await?;
                            self.swarm
                                .behaviour_mut()
                                .request_response
                                .send_response(channel, SwapBytesResponse::Ack)?;
                        }
                        SwapBytesRequest::FileRequest(file_name) => {
                            let mut file_data = Vec::new();
                            if let Ok(mut file) = File::open(&file_name).await {
                                file.read_to_end(&mut file_data).await?;
                            }
                            self.swarm
                                .behaviour_mut()
                                .request_response
                                .send_response(channel, SwapBytesResponse::FileData(file_data))?;
                        }
                    }
                }
                request_response::Message::Response { request_id, response } => {
                    match response {
                        SwapBytesResponse::Ack => {
                            tx.send("[SYSTEM]: Direct message delivered.".to_string()).await?;
                        }
                        SwapBytesResponse::FileData(data) => {
                            if let Some(local_path) = self.pending_file_requests.remove(&request_id) {
                                if !data.is_empty() {
                                    tokio::fs::write(&local_path, &data).await?;
                                    tx.send(format!("[SYSTEM]: File saved to {}", local_path)).await?;
                                } else {
                                    tx.send("[SYSTEM]: File not found or empty.".to_string()).await?;
                                }
                            }
                        }
                    }
                }
            },
            _ => {}
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    let mut stdin = io::BufReader::new(io::stdin()).lines();
    println!("Enter your nickname:");
    let nickname = match stdin.next_line().await {
        Ok(Some(line)) => line.trim().to_string(),
        _ => "Anonymous".to_string(),
    };

    let mut node = SwapBytesNode::new(nickname, cli.bootstrap).await?;
    node.run().await?;
    Ok(())
}