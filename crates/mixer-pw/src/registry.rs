//! A local mirror of the PipeWire graph plus classification and source/device
//! discovery. Single-threaded (lives on the loop thread).

use std::collections::HashMap;

pub type NodeId = u32;
pub const BUS_PREFIX: &str = "ferromix.bus.";
pub const OUR_PREFIX: &str = "ferromix.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    AppPlayback,  // Stream/Output/Audio — a source (produces)
    AppCapture,   // Stream/Input/Audio  — consumes (an app's mic input)
    HwSink,       // Audio/Sink          — a hardware output device
    HwSource,     // Audio/Source        — a hardware input (mic)
    VirtualSrc,   // Audio/Source/Virtual
    OurBus,       // our ferromix.bus.*   null-sink
    OurStrip,     // our ferromix.strip.* null-sink (a strip device)
    Other,
}

#[derive(Debug, Clone)]
pub struct PwNode {
    pub id: NodeId,
    pub name: String,
    pub app_name: Option<String>,
    /// `application.process.binary` — the executable ("discord", "firefox").
    /// This is the ONLY reliable app identity: Discord, Chrome and most
    /// Electron apps all report application.name = "WEBRTC VoiceEngine", so
    /// keying on that would merge them into one unusable entry.
    pub binary: Option<String>,
    pub description: Option<String>,
    pub media_class: String,
}

impl PwNode {
    pub fn kind(&self) -> NodeKind {
        // Our own devices are NEVER hardware. Getting this wrong made A1's
        // "default out" resolve to a FerroMix strip — audio went out of a bus
        // and straight back into a strip, so nothing reached the speakers.
        if self.name.starts_with(BUS_PREFIX) {
            return NodeKind::OurBus;
        }
        if self.name.starts_with(OUR_PREFIX) {
            return NodeKind::OurStrip;
        }
        match self.media_class.as_str() {
            "Stream/Output/Audio" => NodeKind::AppPlayback,
            "Stream/Input/Audio" => NodeKind::AppCapture,
            "Audio/Sink" | "Audio/Duplex" => NodeKind::HwSink,
            "Audio/Source" => NodeKind::HwSource,
            "Audio/Source/Virtual" => NodeKind::VirtualSrc,
            _ => NodeKind::Other,
        }
    }

    /// Stable source key for the UI/config: process binary first, then the
    /// app name, then the node name. Lowercased so config matching is stable.
    pub fn source_key(&self) -> String {
        if let Some(bin) = self.binary.as_deref() {
            let b = bin.trim().to_lowercase();
            if !b.is_empty() {
                return b;
            }
        }
        let n = self.app_name.as_deref().unwrap_or(&self.name);
        n.to_lowercase()
    }

    /// Human label. For apps we title-case the binary ("discord" → "Discord")
    /// because application.name is often junk ("WEBRTC VoiceEngine").
    pub fn label(&self) -> String {
        if matches!(self.kind(), NodeKind::AppPlayback | NodeKind::AppCapture) {
            if let Some(bin) = self.binary.as_deref() {
                let b = bin.trim();
                if !b.is_empty() {
                    return title_case(b);
                }
            }
        }
        self.app_name
            .clone()
            .or_else(|| self.description.clone())
            .unwrap_or_else(|| self.name.clone())
    }
}

#[derive(Debug, Clone)]
pub struct PwPort {
    pub id: u32,
    pub node: NodeId,
    pub dir: Dir,
    pub channel: Option<String>,
    #[allow(dead_code)]
    pub is_monitor: bool,
}

#[derive(Debug, Clone)]
pub struct PwLink {
    pub id: u32,
    pub out_node: NodeId,
    pub in_node: NodeId,
    pub out_port: u32,
    pub in_port: u32,
}

#[derive(Default)]
pub struct Graph {
    pub nodes: HashMap<NodeId, PwNode>,
    pub ports: HashMap<u32, PwPort>,
    pub links: HashMap<u32, PwLink>,
}

impl Graph {
    /// Output ports of a node. For sinks, the "monitor" ports are the outputs;
    /// `want_monitor` selects those for bus→device / metering taps.
    pub fn out_ports(&self, node: NodeId) -> Vec<PwPort> {
        self.ordered(node, Dir::Out)
    }
    pub fn in_ports(&self, node: NodeId) -> Vec<PwPort> {
        self.ordered(node, Dir::In)
    }

    fn ordered(&self, node: NodeId, dir: Dir) -> Vec<PwPort> {
        let mut v: Vec<PwPort> = self
            .ports
            .values()
            .filter(|p| p.node == node && p.dir == dir)
            .cloned()
            .collect();
        let rank = |c: &Option<String>| match c.as_deref() {
            Some("FL") | Some("MONO") => 0,
            Some("FR") => 1,
            _ => 2,
        };
        v.sort_by_key(|p| (rank(&p.channel), p.id));
        v
    }

    pub fn link_exists(&self, out_port: u32, in_port: u32) -> bool {
        self.links.values().any(|l| l.out_port == out_port && l.in_port == in_port)
    }

    #[allow(dead_code)]
    pub fn links_between(&self, out_node: NodeId, in_node: NodeId) -> Vec<u32> {
        self.links
            .values()
            .filter(|l| l.out_node == out_node && l.in_node == in_node)
            .map(|l| l.id)
            .collect()
    }

    /// Does `in_node` receive audio from `out_node` (directly)? Used for
    /// feedback detection (app captures from a bus it is also feeding).
    pub fn has_link(&self, out_node: NodeId, in_node: NodeId) -> bool {
        self.links.values().any(|l| l.out_node == out_node && l.in_node == in_node)
    }
}

pub fn parse_node(id: u32, props: &pipewire::spa::utils::dict::DictRef) -> Option<PwNode> {
    let media_class = props.get("media.class")?.to_string();
    if !media_class.contains("Audio") {
        return None;
    }
    Some(PwNode {
        id,
        name: props.get("node.name").unwrap_or("").to_string(),
        app_name: props.get("application.name").map(str::to_string),
        binary: props
            .get("application.process.binary")
            .map(|b| {
                // strip any path, keep the executable name
                b.rsplit('/').next().unwrap_or(b).to_string()
            }),
        description: props.get("node.description").map(str::to_string),
        media_class,
    })
}

pub fn parse_port(id: u32, props: &pipewire::spa::utils::dict::DictRef) -> Option<PwPort> {
    let node: NodeId = props.get("node.id")?.parse().ok()?;
    let dir = match props.get("port.direction")? {
        "in" => Dir::In,
        "out" => Dir::Out,
        _ => return None,
    };
    let is_monitor = props.get("port.monitor") == Some("true")
        || props.get("audio.channel").is_some() && props.get("port.name").map(|n| n.starts_with("monitor")).unwrap_or(false);
    Some(PwPort { id, node, dir, channel: props.get("audio.channel").map(str::to_string), is_monitor })
}

pub fn parse_link(id: u32, props: &pipewire::spa::utils::dict::DictRef) -> Option<PwLink> {
    Some(PwLink {
        id,
        out_node: props.get("link.output.node")?.parse().ok()?,
        in_node: props.get("link.input.node")?.parse().ok()?,
        out_port: props.get("link.output.port")?.parse().ok()?,
        in_port: props.get("link.input.port")?.parse().ok()?,
    })
}

/// "discord" → "Discord", "firefox" → "Firefox", "linphone-desktop" → "Linphone Desktop"
fn title_case(s: &str) -> String {
    s.split(|c| c == '-' || c == '_' || c == '.')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
