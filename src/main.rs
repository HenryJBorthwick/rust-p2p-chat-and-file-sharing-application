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
use tokio::time::interval; // Fixed: Removed unused Interval

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
        let mut stdin = io::BufReader::new(io::stdin()).lines();
        let mut retry_interval = interval(Duration::from_secs(5));

        loop {
            select! {
                line_result = stdin.next_line() => {
                    if let Ok(Some(line)) = line_result {
                        self.handle_input(&line).await?;
                    }
                }
                event = self.swarm.select_next_some() => {
                    self.handle_event(event).await?;
                }
                _ = retry_interval.tick(), if !self.nickname_announced => {
                    match self.announce_nickname().await {
                        Ok(_) => {
                            self.nickname_announced = true;
                            println!("Nickname '{}' announced successfully.", self.nickname);
                        }
                        Err(e) if e.to_string().contains("InsufficientPeers") => {
                            println!("Still insufficient peers for Gossipsub. Retrying in 5 seconds...");
                        }
                        Err(e) => {
                            println!("Error announcing nickname: {}. Will retry.", e);
                        }
                    }
                }
            }
        }
    }

    async fn handle_input(&mut self, line: &str) -> Result<(), Box<dyn Error>> {
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
                        Ok(_) => {}
                        Err(gossipsub::PublishError::InsufficientPeers) => {
                            println!("No peers connected yet. Please wait for peers to be discovered.");
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
                            .send_request(&target_peer, SwapBytesRequest::DirectMessage(message));
                    } else {
                        println!("Nickname '{}' not found.", target_nickname);
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
                    } else {
                        println!("Nickname '{}' not found.", target_nickname);
                    }
                } else {
                    println!("Usage: /getfile <nickname> <file_name> <local_path>");
                }
            }
            "/list" => {
                println!("Known peers:");
                for (peer_id, nick) in &self.peer_nicknames {
                    println!("- {}: {}", nick, peer_id);
                }
            }
            _ => println!("Unknown command: {}", parts[0]),
        }
        Ok(())
    }

    async fn handle_event(&mut self, event: SwarmEvent<SwapBytesBehaviourEvent>) -> Result<(), Box<dyn Error>> {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on {}", address);
            }
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                for (peer_id, multiaddr) in list {
                    println!("Discovered peer: {} at {}", peer_id, multiaddr);
                    self.swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr.clone());
                    self.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("Connected to peer: {}", peer_id);
            }
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message,
                ..
            })) => {
                let msg = String::from_utf8_lossy(&message.data);
                if msg.starts_with("JOIN ") {
                    let nickname = msg.trim_start_matches("JOIN ").to_string();
                    self.peer_nicknames.insert(propagation_source, nickname.clone());
                    println!("{} ({}) joined the chat.", nickname, propagation_source);
                } else {
                    if let Some(n) = self.peer_nicknames.get(&propagation_source) {
                        println!("Chat [{}]: {}", n, msg);
                    } else {
                        let prop_str = propagation_source.to_string();
                        println!("Chat [{}]: {}", prop_str, msg);
                    }
                }
            }
            SwarmEvent::Behaviour(SwapBytesBehaviourEvent::RequestResponse(
                request_response::Event::Message { peer, message, .. }
            )) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    match request {
                        SwapBytesRequest::DirectMessage(msg) => {
                            println!("DM from {}: {}", self.peer_nicknames.get(&peer).unwrap_or(&peer.to_string()), msg);
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
                            println!("Direct message delivered.");
                        }
                        SwapBytesResponse::FileData(data) => {
                            if let Some(local_path) = self.pending_file_requests.remove(&request_id) {
                                if !data.is_empty() {
                                    tokio::fs::write(&local_path, &data).await?;
                                    println!("File saved to {}", local_path);
                                } else {
                                    println!("File not found or empty.");
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
    println!("Commands:");
    println!("- /chat <message>");
    println!("- /dm <nickname> <message>");
    println!("- /getfile <nickname> <file_name> <local_path>");
    println!("- /list");
    node.run().await?;
    Ok(())
}