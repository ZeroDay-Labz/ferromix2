//! The PipeWire loop thread: registry mirror, bus creation/adoption, source &
//! device discovery, and the declarative reconciler.
//!
//! Model: SOURCES (mic + app playbacks) are assigned to BUSES (A = null-sinks
//! whose monitor drives a hardware device; B = virtual sources apps use as a
//! mic). Assigning an app to any bus REDIRECTS it off its default sink
//! (Voicemeeter behavior — audio comes out where you route it, not twice).
//! Feedback guard: an app-out assigned to a B-bus it also listens to is
//! refused, so mix-minus can't arm an echo.

use crate::registry::{self, Graph, NodeId, NodeKind};
use crate::{links, recorder, tap, virtual_dev, PwCmd};
use mixer_core::backend::BackendEvent;
use mixer_core::model::{self as m, BusKind, Device, LevelKey, RecTarget, SourceInfo, SourceKind};
use pipewire as pw;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::Sender;

/// Every strip is a virtual sink device named `ferromix.strip.N`, described to
/// the system as "FerroMix Input N+1". Apps can point their output straight at
/// it; a mic or an app can also be linked into it. Either way all audio for the
/// strip passes through this one node — which is what makes the fader, mute and
/// meter work uniformly for every kind of input.
fn strip_name(idx: usize) -> String {
    format!("ferromix.strip.{idx}")
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Slot {
    /// source -> strip device
    StripIn(usize),
    /// bus monitor -> A-bus (hear what the far end hears)
    Monitor(usize, usize),
    /// strip device monitor -> bus
    Send(usize, usize),
    /// bus monitor -> hardware sink
    BusDev(usize),
    /// B-bus output -> an app's capture (mic) input. We draw this link
    /// OURSELVES instead of asking WirePlumber via target.object, because the
    /// B-bus is created with node.autoconnect=false — WirePlumber refuses to
    /// route a node it's told to leave alone, which stranded the app's mic and
    /// let it fall back to grabbing the raw default source. Drawing the link
    /// directly sidesteps WirePlumber entirely.
    BusListener(usize, NodeId),
}

#[derive(Default)]
struct Desired {
    strips: HashMap<usize, String>,
    /// strip -> the source key linked into it (a mic or an app).
    strip_input: HashMap<usize, String>,
    strip_vol: HashMap<usize, f32>,
    strip_mute: HashMap<usize, bool>,
    /// (strip, bus) sends.
    assigns: HashSet<(usize, usize)>,
    buses: HashMap<usize, (String, BusKind)>,
    bus_dev: HashMap<usize, Option<String>>,
    bus_vol: HashMap<usize, f32>,
    bus_mute: HashMap<usize, bool>,
}

#[derive(Default)]
struct WorkerState {
    graph: Graph,
    desired: Desired,
    bus_nodes: HashMap<usize, (NodeId, pw::node::Node)>,
    strip_nodes: HashMap<usize, (NodeId, pw::node::Node)>,
    /// Any ferromix.* device seen in the registry, awaiting adoption.
    bus_by_name: HashMap<String, (NodeId, pw::node::Node)>,
    feedback_guard: bool,
    last_feedback_pairs: Vec<(usize, usize)>,
    last_listeners: Vec<(usize, Vec<String>)>,
    /// The PipeWire "default" metadata object — how `wpctl set-default` and
    /// `pw-metadata <id> target.object` work.
    metadata: Option<pw::metadata::Metadata>,
    /// app node -> strip device we have already pointed it at (avoids spam).
    stream_targets: HashMap<NodeId, String>,
    /// Initial registry enumeration finished (safe to create devices).
    synced: bool,
    /// Commands parked until the initial sync completes.
    parked: Vec<PwCmd>,
    creating: HashSet<String>,
    keepalive: Vec<pw::node::Node>,
    bound: HashMap<NodeId, pw::node::Node>,
    link_proxies: HashMap<Slot, Vec<((u32, u32), pw::link::Link)>>,
    taps: HashMap<LevelKey, tap::Tap>,
    recorders: HashMap<RecTarget, recorder::Recorder>,
    /// (bus, a_bus) monitor sends: hear what a virtual mic is sending.
    monitors: HashSet<(usize, usize)>,
    /// bus -> app key whose MICROPHONE we point at it.
    bus_listener: HashMap<usize, String>,
    /// capture node -> bus name we already pointed it at.
    capture_targets: HashMap<NodeId, String>,
    /// (bus idx, capture node) -> unit, tracking B-bus→app-mic links we drew
    /// ourselves so we can tear them down when the assignment changes.
    listener_links: HashMap<(usize, NodeId), ()>,
    last_capture_apps: Vec<SourceInfo>,
    last_sources: Vec<SourceInfo>,
    last_devices: Vec<Device>,
}

#[derive(Clone)]
struct Ctx {
    // 0.9's `*Rc` handles are themselves reference-counted, so we no longer
    // wrap them in std::rc::Rc.
    core: pw::core::CoreRc,
    registry: pw::registry::RegistryRc,
    ev_tx: Sender<BackendEvent>,
    st: Rc<RefCell<WorkerState>>,
}

impl Ctx {
    fn log(&self, m: impl Into<String>) {
        let _ = self.ev_tx.send(BackendEvent::Log(m.into()));
    }
}

pub(crate) fn run(cmd_rx: pw::channel::Receiver<PwCmd>, ev_tx: Sender<BackendEvent>) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("mainloop: {e}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| format!("context: {e}"))?;
    let core = context.connect_rc(None).map_err(|e| format!("connect: {e}"))?;
    let registry = core.get_registry_rc().map_err(|e| format!("registry: {e}"))?;

    let ctx = Ctx {
        core,
        registry: registry.clone(),
        ev_tx: ev_tx.clone(),
        st: Rc::new(RefCell::new(WorkerState { feedback_guard: true, ..Default::default() })),
    };

    let _rl = registry
        .add_listener_local()
        .global({
            let ctx = ctx.clone();
            move |g| on_global(&ctx, g)
        })
        .global_remove({
            let ctx = ctx.clone();
            move |id| on_global_remove(&ctx, id)
        })
        .register();

    let _cg = cmd_rx.attach(mainloop.loop_(), {
        let ctx = ctx.clone();
        move |cmd| {
            let synced = ctx.st.borrow().synced;
            if synced {
                handle_cmd(&ctx, cmd);
            } else {
                // Creating devices before the initial enumeration lands could
                // duplicate lingering nodes from a previous run — park it.
                ctx.st.borrow_mut().parked.push(cmd);
            }
        }
    });

    // Mark the end of the initial registry dump with a core sync roundtrip.
    let _core_listener = ctx
        .core
        .add_listener_local()
        .done({
            let ctx = ctx.clone();
            move |_id, _seq| {
                let already = ctx.st.borrow().synced;
                if already {
                    return;
                }
                ctx.st.borrow_mut().synced = true;
                cleanup_stale(&ctx);
                ensure_all_strips(&ctx);
                let parked: Vec<PwCmd> = std::mem::take(&mut ctx.st.borrow_mut().parked);
                for cmd in parked {
                    handle_cmd(&ctx, cmd);
                }
                ctx.log("initial graph sync complete");
            }
        })
        .register();
    let _ = ctx.core.sync(0);

    ctx.log("pipewire connected");
    mainloop.run();
    Ok(())
}

fn on_global(ctx: &Ctx, g: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>) {
    match g.type_ {
        pw::types::ObjectType::Node => {
            let Some(props) = g.props else { return };
            let Some(node) = registry::parse_node(g.id, props) else { return };
            let kind = node.kind();

            let ours = node.name.starts_with("ferromix.")
                && !node.name.contains(".tap.")
                && !node.name.contains(".rec.");
            if ours {
                if let Ok(bound) = ctx.registry.bind::<pw::node::Node, _>(g) {
                    ctx.st.borrow_mut().bus_by_name.insert(node.name.clone(), (node.id, bound));
                }
            } else if matches!(kind, NodeKind::AppPlayback | NodeKind::HwSource | NodeKind::AppCapture) {
                // Bind so we can control volume/mute on it later.
                if let Ok(bound) = ctx.registry.bind::<pw::node::Node, _>(g) {
                    ctx.st.borrow_mut().bound.insert(node.id, bound);
                }
            }
            ctx.st.borrow_mut().graph.nodes.insert(node.id, node);
            adopt_buses(ctx);
            adopt_strips(ctx);
            apply_strip_controls(ctx);
            emit_inventory(ctx);
            emit_capture_apps(ctx);
            apply_bus_listeners(ctx);
            reconcile(ctx);
        }
        pw::types::ObjectType::Metadata => {
            let name = g.props.and_then(|p| p.get("metadata.name")).unwrap_or("");
            if name == "default" && ctx.st.borrow().metadata.is_none() {
                match ctx.registry.bind::<pw::metadata::Metadata, _>(g) {
                    Ok(md) => {
                        ctx.st.borrow_mut().metadata = Some(md);
                        log::info!("bound default metadata — can set system default devices");
                    }
                    Err(e) => log::warn!("bind metadata: {e}"),
                }
            }
        }
        pw::types::ObjectType::Port => {
            let Some(props) = g.props else { return };
            if let Some(port) = registry::parse_port(g.id, props) {
                ctx.st.borrow_mut().graph.ports.insert(port.id, port);
                reconcile(ctx);
            }
        }
        pw::types::ObjectType::Link => {
            let Some(props) = g.props else { return };
            if let Some(link) = registry::parse_link(g.id, props) {
                ctx.st.borrow_mut().graph.links.insert(link.id, link);
                reconcile(ctx);
                emit_listeners(ctx);
            }
        }
        _ => {}
    }
}

fn on_global_remove(ctx: &Ctx, id: u32) {
    {
        let mut st = ctx.st.borrow_mut();
        if let Some(node) = st.graph.nodes.remove(&id) {
            st.bound.remove(&id);
            st.stream_targets.remove(&id);
            st.capture_targets.remove(&id);
            st.bus_by_name.remove(&node.name);
            let gone: Vec<usize> = st.bus_nodes.iter().filter(|(_, (nid, _))| *nid == id).map(|(i, _)| *i).collect();
            for i in gone {
                st.bus_nodes.remove(&i);
                st.taps.remove(&LevelKey::Bus(i));
            }
            let gone_strip: Vec<usize> =
                st.strip_nodes.iter().filter(|(_, (nid, _))| *nid == id).map(|(i, _)| *i).collect();
            for i in gone_strip {
                st.strip_nodes.remove(&i);
                st.taps.remove(&LevelKey::Strip(i));
            }
        }
        st.graph.ports.remove(&id);
        st.graph.links.remove(&id);
    }
    emit_inventory(ctx);
    reconcile(ctx);
}

/// Create/adopt bus null-sinks for every desired bus.
fn adopt_buses(ctx: &Ctx) {
    let mut ready: Vec<(usize, NodeId)> = Vec::new();
    {
        let mut st = ctx.st.borrow_mut();
        let wanted: Vec<(usize, String, BusKind)> =
            st.desired.buses.iter().filter(|(i, _)| !st.bus_nodes.contains_key(i)).map(|(i, (l, k))| (*i, l.clone(), *k)).collect();
        for (idx, _label, kind) in wanted {
            let name = virtual_dev::bus_node_name(idx);
            if let Some((id, node)) = st.bus_by_name.remove(&name) {
                st.creating.remove(&name);
                if let Some(v) = st.desired.bus_vol.get(&idx).copied() {
                    let _ = virtual_dev::set_node_volume(&node, v);
                }
                if let Some(m) = st.desired.bus_mute.get(&idx).copied() {
                    let _ = virtual_dev::set_node_mute(&node, m);
                }
                let cap_sink = kind == BusKind::HwOutput; // sink monitor vs virtual source out
                if let Ok(t) = tap::Tap::new(&ctx.core, LevelKey::Bus(idx), &name, cap_sink, ctx.ev_tx.clone()) {
                    st.taps.insert(LevelKey::Bus(idx), t);
                }
                st.bus_nodes.insert(idx, (id, node));
                ready.push((idx, id));
            }
        }
    }
    for (idx, id) in ready {
        let _ = ctx.ev_tx.send(BackendEvent::BusReady { idx, id });
    }
}

/// Destroy any `ferromix.*` node found during the initial enumeration.
///
/// Our devices are not lingered, so they die with the daemon. Anything of ours
/// still present at startup is therefore a leftover — either from an older
/// build that DID linger (which is what produced "FerroMix A1-1" duplicates),
/// or from a crashed run. Sweeping them guarantees exactly one node per bus.
fn cleanup_stale(ctx: &Ctx) {
    let stale: Vec<(u32, String)> = {
        let st = ctx.st.borrow();
        st.graph
            .nodes
            .values()
            .filter(|n| n.name.starts_with("ferromix."))
            .map(|n| (n.id, n.name.clone()))
            .collect()
    };
    if stale.is_empty() {
        return;
    }
    ctx.log(format!("sweeping {} stale FerroMix node(s) from a previous run", stale.len()));
    {
        // Drop bound proxies first so destroy_global isn't racing our handles.
        let mut st = ctx.st.borrow_mut();
        st.bus_by_name.clear();
        for (id, _) in &stale {
            st.graph.nodes.remove(id);
        }
    }
    for (id, name) in stale {
        log::info!("destroying stale node {id} ({name})");
        let _ = ctx.registry.destroy_global(id);
    }
}

/// Claim strip device nodes that have shown up in the registry.
fn adopt_strips(ctx: &Ctx) {
    let mut ready: Vec<(usize, NodeId)> = Vec::new();
    {
        let mut st = ctx.st.borrow_mut();
        let wanted: Vec<usize> =
            st.desired.strips.keys().copied().filter(|i| !st.strip_nodes.contains_key(i)).collect();
        for idx in wanted {
            let name = strip_name(idx);
            let Some((id, node)) = st.bus_by_name.remove(&name) else { continue };
            st.creating.remove(&name);
            if let Some(v) = st.desired.strip_vol.get(&idx).copied() {
                let _ = virtual_dev::set_node_volume(&node, v);
            }
            if let Some(mu) = st.desired.strip_mute.get(&idx).copied() {
                let _ = virtual_dev::set_node_mute(&node, mu);
            }
            // A strip is a sink: meter its monitor.
            if let Ok(t) = tap::Tap::new(&ctx.core, LevelKey::Strip(idx), &name, true, ctx.ev_tx.clone()) {
                st.taps.insert(LevelKey::Strip(idx), t);
            }
            st.strip_nodes.insert(idx, (id, node));
            ready.push((idx, id));
        }
    }
    for (idx, id) in ready {
        let _ = ctx.ev_tx.send(BackendEvent::StripReady { idx, id });
        ctx.log(format!("{} ready — apps can select it as their output", m::strip_device_label(idx)));
    }
}

fn ensure_strip_device(ctx: &Ctx, idx: usize) {
    adopt_strips(ctx);
    let name = strip_name(idx);
    {
        let st = ctx.st.borrow();
        if st.strip_nodes.contains_key(&idx)
            || st.creating.contains(&name)
            || st.bus_by_name.contains_key(&name)
        {
            return;
        }
    }
    ctx.st.borrow_mut().creating.insert(name.clone());
    match virtual_dev::create_sink(&ctx.core, &name, &m::strip_device_label(idx)) {
        Ok(node) => ctx.st.borrow_mut().keepalive.push(node),
        Err(e) => {
            ctx.st.borrow_mut().creating.remove(&name);
            ctx.log(format!("create {} FAILED: {e}", m::strip_device_label(idx)));
        }
    }
}

fn ensure_all_strips(ctx: &Ctx) {
    let idxs: Vec<usize> = ctx.st.borrow().desired.strips.keys().copied().collect();
    for i in idxs {
        ensure_strip_device(ctx, i);
    }
}

fn ensure_bus_device(ctx: &Ctx, idx: usize, label: &str, kind: BusKind) {
    adopt_buses(ctx);
    let need_create = {
        let st = ctx.st.borrow();
        !st.bus_nodes.contains_key(&idx)
            && !st.creating.contains(&virtual_dev::bus_node_name(idx))
            && !st.bus_by_name.contains_key(&virtual_dev::bus_node_name(idx))
    };
    if !need_create {
        return;
    }
    let name = virtual_dev::bus_node_name(idx);
    ctx.st.borrow_mut().creating.insert(name.clone());
    let desc = format!("FerroMix {label}");
    let created = match kind {
        BusKind::HwOutput => virtual_dev::create_sink(&ctx.core, &name, &desc),
        BusKind::VirtualMic => virtual_dev::create_virtual_source(&ctx.core, &name, &desc),
    };
    match created {
        Ok(node) => {
            ctx.st.borrow_mut().keepalive.push(node);
            ctx.log(format!("created bus {label}"));
        }
        Err(e) => {
            ctx.st.borrow_mut().creating.remove(&name);
            ctx.log(format!("create bus {label} FAILED: {e}"));
        }
    }
}

/// Resolve a source key to the live node ids backing it.
fn resolve_source(st: &WorkerState, key: &str) -> Vec<NodeId> {
    let key_l = key.to_lowercase();
    st.graph
        .nodes
        .values()
        .filter(|n| {
            matches!(n.kind(), NodeKind::AppPlayback | NodeKind::HwSource) && {
                let k = n.source_key();
                k == key_l || k.contains(&key_l)
            }
        })
        .map(|n| n.id)
        .collect()
}

/// Faders and mutes act on the strip's own device node — never on the app or
/// the mic, so they behave identically for every kind of input.
/// Capture (microphone) stream nodes belonging to an app.
fn resolve_capture(st: &WorkerState, key: &str) -> Vec<NodeId> {
    let key_l = key.to_lowercase();
    st.graph
        .nodes
        .values()
        .filter(|n| n.kind() == NodeKind::AppCapture && n.source_key() == key_l)
        .map(|n| n.id)
        .collect()
}

/// Point the assigned app's microphone at each B bus, using the same
/// `target.object` metadata trick we use for playback. This is what lets you
/// set Discord's mic to B1 from inside FerroMix instead of digging through
/// Discord's own settings.
fn apply_bus_listeners(ctx: &Ctx) {
    // Draw B-bus -> app-capture links ourselves. WirePlumber won't route into a
    // node marked node.autoconnect=false, so target.object silently failed and
    // the app's mic fell back to the default source (the Spotify+mic bleed).
    // Collect the desired (bus_node, capture_node) pairs.
    let desired: Vec<(usize, NodeId, NodeId)> = {
        let st = ctx.st.borrow();
        let mut v = Vec::new();
        for (bus, key) in st.bus_listener.iter() {
            // A muted B-bus sends nothing — skip its listener link entirely.
            if st.desired.bus_mute.get(bus).copied().unwrap_or(false) {
                continue;
            }
            let Some(&(bus_node, _)) = st.bus_nodes.get(bus) else { continue };
            for cap in resolve_capture(&st, key) {
                v.push((*bus, bus_node, cap));
            }
        }
        v
    };

    // Remove any stale listener links whose app is no longer assigned.
    let stale: Vec<(usize, NodeId)> = {
        let st = ctx.st.borrow();
        st.listener_links
            .keys()
            .copied()
            .filter(|(bus, cap)| !desired.iter().any(|(b, _, c)| b == bus && c == cap))
            .collect()
    };
    for (bus, cap) in stale {
        remove_slot_links(ctx, &Slot::BusListener(bus, cap));
        ctx.st.borrow_mut().listener_links.remove(&(bus, cap));
    }

    // Ensure the wanted links exist.
    for (bus, bus_node, cap) in desired {
        let already = ctx.st.borrow().listener_links.contains_key(&(bus, cap));
        if !already {
            let mut st = ctx.st.borrow_mut();
            ensure_links(&ctx.core, &mut st, Slot::BusListener(bus, cap), bus_node, cap);
            st.listener_links.insert((bus, cap), ());
            log::info!("MIC LINK bus.{bus} -> {}", nname(&st, cap));
        }
    }
}

/// Apps with a live microphone stream — the ones you can hand a B bus to.
fn emit_capture_apps(ctx: &Ctx) {
    let apps: Vec<SourceInfo> = {
        let st = ctx.st.borrow();
        let mut seen = HashSet::new();
        let mut v: Vec<SourceInfo> = Vec::new();
        let mut nodes: Vec<&registry::PwNode> =
            st.graph.nodes.values().filter(|n| n.kind() == NodeKind::AppCapture).collect();
        nodes.sort_by_key(|n| n.id);
        for n in nodes {
            let key = n.source_key();
            if seen.insert(key.clone()) {
                v.push(SourceInfo { key, label: n.label(), kind: SourceKind::App });
            }
        }
        v
    };
    let mut st = ctx.st.borrow_mut();
    if apps != st.last_capture_apps {
        st.last_capture_apps = apps.clone();
        let _ = ctx.ev_tx.send(BackendEvent::CaptureAppsChanged(apps));
    }
}

fn apply_strip_controls(ctx: &Ctx) {
    let st = ctx.st.borrow();
    for (idx, v) in st.desired.strip_vol.iter() {
        if let Some((_, node)) = st.strip_nodes.get(idx) {
            let _ = virtual_dev::set_node_volume(node, *v);
        }
    }
    for (idx, mu) in st.desired.strip_mute.iter() {
        if let Some((_, node)) = st.strip_nodes.get(idx) {
            let _ = virtual_dev::set_node_mute(node, *mu);
        }
    }
}

/// Who is capturing from each B bus? An app that has selected "FerroMix B1"
/// as its microphone shows up here — that's how you confirm Discord actually
/// took the virtual mic, rather than guessing.
fn emit_listeners(ctx: &Ctx) {
    let listeners: Vec<(usize, Vec<String>)> = {
        let st = ctx.st.borrow();
        let mut out = Vec::new();
        for (idx, (node, _)) in st.bus_nodes.iter() {
            let mut who: Vec<String> = st
                .graph
                .links
                .values()
                .filter(|l| l.out_node == *node)
                .filter_map(|l| st.graph.nodes.get(&l.in_node))
                .filter(|n| n.kind() == NodeKind::AppCapture)
                .map(|n| n.label())
                .collect();
            who.sort();
            who.dedup();
            out.push((*idx, who));
        }
        out.sort();
        out
    };
    let mut st = ctx.st.borrow_mut();
    if listeners != st.last_listeners {
        st.last_listeners = listeners.clone();
        let _ = ctx.ev_tx.send(BackendEvent::BusListeners(listeners));
    }
}

/// Rebuild + emit the source and device inventories if they changed.
fn emit_inventory(ctx: &Ctx) {
    let (sources, devices) = {
        let st = ctx.st.borrow();
        // Sources: Virtual Input, every hardware capture device, then apps.
        let mut sources: Vec<SourceInfo> = Vec::new();
        let mut hw_sources: Vec<&registry::PwNode> =
            st.graph.nodes.values().filter(|n| n.kind() == NodeKind::HwSource).collect();
        hw_sources.sort_by_key(|n| n.id);
        let mut seen_hw = HashSet::new();
        for n in hw_sources.iter() {
            let key = n.source_key();
            if seen_hw.insert(key.clone()) {
                sources.push(SourceInfo { key, label: n.label(), kind: SourceKind::HwInput });
            }
        }
        let mut seen = HashSet::new();
        let mut apps: Vec<&registry::PwNode> =
            st.graph.nodes.values().filter(|n| n.kind() == NodeKind::AppPlayback).collect();
        apps.sort_by_key(|n| n.id);
        for n in apps {
            let key = n.source_key();
            if seen.insert(key.clone()) {
                sources.push(SourceInfo { key, label: n.label(), kind: SourceKind::App });
            }
        }
        // Devices: hardware sinks (not ours).
        let mut devices: Vec<Device> = st
            .graph
            .nodes
            .values()
            .filter(|n| n.kind() == NodeKind::HwSink && !n.name.starts_with(registry::OUR_PREFIX))
            .map(|n| Device { key: n.name.clone(), label: n.label() })
            .collect();
        devices.sort_by(|a, b| a.label.cmp(&b.label));
        (sources, devices)
    };

    let mut st = ctx.st.borrow_mut();
    if sources != st.last_sources {
        st.last_sources = sources.clone();
        let _ = ctx.ev_tx.send(BackendEvent::SourcesChanged(sources));
    }
    if devices != st.last_devices {
        st.last_devices = devices.clone();
        let _ = ctx.ev_tx.send(BackendEvent::DevicesChanged(devices));
    }
}

/// Which app "owns" strip `idx`? Either the source we linked in, or whatever
/// app has pointed its own output at the strip's device.
/// The app keys that legitimately OWN a strip: the app FerroMix deliberately
/// routed there (its configured input), not whatever transiently shows up
/// linked to the strip device. Keying off live links let a bootstrapping app
/// (e.g. Discord's playback momentarily passing through) register as a phantom
/// owner of an unrelated strip — which then tripped the feedback guard on a
/// perfectly safe send like mic→B2.
fn owner_keys_of(
    graph: &Graph,
    strip_input: &HashMap<usize, String>,
    idx: usize,
) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    // 1. The configured input for this strip is the authoritative owner.
    if let Some(k) = strip_input.get(&idx) {
        keys.push(k.to_lowercase());
    }
    // 2. Also honour an app that pointed ITS OWN playback at this strip via
    //    metadata — but only playback streams whose source_key matches the
    //    configured input, so a stray Electron stream can't claim the strip.
    let want = strip_input.get(&idx).map(|k| k.to_lowercase());
    if let Some(want) = want {
        for n in graph.nodes.values() {
            if n.kind() == NodeKind::AppPlayback && n.source_key() == want {
                keys.push(n.source_key());
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// Would sending strip `idx` into `bus_node` echo? True only when an app that
/// legitimately owns this strip is ALSO capturing from that bus (i.e. the app
/// would hear itself). mic→B2 is safe even if Discord captures B2, because the
/// mic strip is not owned by Discord.
fn strip_listens_to_bus(st: &WorkerState, idx: usize, bus_node: NodeId) -> bool {
    // Name-based check (fast path): an app owning this strip captures the bus.
    if listens(&st.graph, &st.desired.strip_input, idx, bus_node) {
        return true;
    }
    // Structural check (catches apps whose stream names don't match their
    // configured key — e.g. a SIP softphone). Trace the real links:
    //   bus --captured_by--> APP --its_playback--> this strip?
    // If the same app both reads this bus as its mic AND plays back onto this
    // strip, then sending the strip into the bus loops the app to itself.
    structural_feedback(&st.graph, st.strip_nodes.get(&idx).map(|(n, _)| *n), bus_node)
}

/// Follow actual graph links to detect an app-mediated loop, independent of any
/// name/key matching. Returns true if some app captures `bus_node` AND that same
/// app's playback is linked into `strip_node`.
fn structural_feedback(graph: &Graph, strip_node: Option<NodeId>, bus_node: NodeId) -> bool {
    let Some(strip_node) = strip_node else { return false };

    // Apps whose CAPTURE stream reads this bus (their microphone = this bus).
    let capturing_apps: Vec<String> = graph
        .nodes
        .values()
        .filter(|n| n.kind() == NodeKind::AppCapture && graph.has_link(bus_node, n.id))
        .map(|n| n.source_key())
        .collect();
    if capturing_apps.is_empty() {
        return false;
    }

    // Does any of those same apps have a PLAYBACK stream feeding this strip?
    graph
        .nodes
        .values()
        .filter(|n| n.kind() == NodeKind::AppPlayback && graph.has_link(n.id, strip_node))
        .any(|pb| capturing_apps.contains(&pb.source_key()))
}

fn listens(
    graph: &Graph,
    strip_input: &HashMap<usize, String>,
    idx: usize,
    bus_node: NodeId,
) -> bool {
    let owners = owner_keys_of(graph, strip_input, idx);
    if owners.is_empty() {
        return false;
    }
    graph
        .nodes
        .values()
        .filter(|n| n.kind() == NodeKind::AppCapture && owners.contains(&n.source_key()))
        .any(|cap| graph.has_link(bus_node, cap.id))
}

fn reconcile(ctx: &Ctx) {
    let mut feedback: Vec<(usize, usize)> = Vec::new();
    {
        let mut st = ctx.st.borrow_mut();
        let st = &mut *st;

        // 1. source -> strip device
        let inputs: Vec<(usize, String)> =
            st.desired.strip_input.iter().map(|(i, k)| (*i, k.clone())).collect();
        for (idx, key) in inputs {
            let Some(&(strip_node, _)) = st.strip_nodes.get(&idx) else { continue };
            for src in resolve_source(st, &key) {
                ensure_links(&ctx.core, st, Slot::StripIn(idx), src, strip_node);
            }
        }

        // 2. Point each app that belongs on a strip AT that strip's device,
        //    using PipeWire metadata (`target.object`) — the same mechanism
        //    `pw-metadata <id> target.object <name>` uses.
        //
        //    We must NOT destroy the app's existing links: WirePlumber owns
        //    stream placement and instantly recreates them, which turns into an
        //    endless destroy/recreate war (it did — hundreds of times a second).
        //    Setting the target tells the session manager where the stream
        //    belongs, and it moves it and keeps it there.
        let targets: Vec<(NodeId, String)> = {
            let mut v = Vec::new();
            for (idx, key) in st.desired.strip_input.iter() {
                let Some(&(_, _)) = st.strip_nodes.get(idx) else { continue };
                let name = strip_name(*idx);
                for src in resolve_source(st, key) {
                    if st.graph.nodes.get(&src).map(|n| n.kind()) == Some(NodeKind::AppPlayback) {
                        v.push((src, name.clone()));
                    }
                }
            }
            v
        };
        for (node, target) in targets {
            if st.stream_targets.get(&node) == Some(&target) {
                continue; // already pointed there
            }
            st.stream_targets.insert(node, target.clone());
            if let Some(md) = st.metadata.as_ref() {
                let json = format!("\"{target}\"");
                md.set_property(node, "target.object", Some("Spa:String:JSON"), Some(&json));
                log::info!("TARGET {} -> {}", nname(st, node), target);
            } else {
                log::warn!("cannot retarget {}: no metadata object", nname(st, node));
            }
        }

        // 3. strip -> bus sends (with the feedback guard AND mute)
        let assigns: Vec<(usize, usize)> = st.desired.assigns.iter().copied().collect();
        for (sidx, bidx) in assigns {
            let (Some(&(strip_node, _)), Some(&(bus_node, _))) =
                (st.strip_nodes.get(&sidx), st.bus_nodes.get(&bidx))
            else {
                continue;
            };
            // MUTE means the strip sends NOWHERE. We cut the actual links rather
            // than only muting the node, because a null-sink's monitor keeps
            // emitting into a B bus even when the sink itself is muted — that was
            // the "muted mic still reaches Discord" bug. A muted strip = no links.
            if st.desired.strip_mute.get(&sidx).copied().unwrap_or(false) {
                remove_links_between(ctx, st, strip_node, bus_node);
                continue;
            }
            let kind = st.desired.buses.get(&bidx).map(|(_, k)| *k).unwrap_or(BusKind::HwOutput);
            if st.feedback_guard
                && kind == BusKind::VirtualMic
                && strip_listens_to_bus(st, sidx, bus_node)
            {
                feedback.push((sidx, bidx));
                remove_links_between(ctx, st, strip_node, bus_node);
                continue;
            }
            ensure_links(&ctx.core, st, Slot::Send(sidx, bidx), strip_node, bus_node);
        }

        // 3b. Bus monitoring: send a virtual mic into a hardware out so you can
        //     hear exactly what you're sending to the far end.
        let mons: Vec<(usize, usize)> = st.monitors.iter().copied().collect();
        for (b, a) in mons {
            let (Some(&(src, _)), Some(&(dst, _))) = (st.bus_nodes.get(&b), st.bus_nodes.get(&a))
            else {
                continue;
            };
            ensure_links(&ctx.core, st, Slot::Monitor(b, a), src, dst);
        }

        // 4. A-bus -> hardware device
        let hw_buses: Vec<usize> = st
            .desired
            .buses
            .iter()
            .filter(|(_, (_, k))| *k == BusKind::HwOutput)
            .map(|(i, _)| *i)
            .collect();
        for b in hw_buses {
            let Some(&(bus_node, _)) = st.bus_nodes.get(&b) else { continue };
            let target = st.desired.bus_dev.get(&b).cloned().flatten();
            if let Some(hw) = find_hw_sink(st, target.as_deref()) {
                ensure_links(&ctx.core, st, Slot::BusDev(b), bus_node, hw);
            }
        }
    }

    let mut st = ctx.st.borrow_mut();
    feedback.sort();
    if feedback != st.last_feedback_pairs {
        st.last_feedback_pairs = feedback.clone();
        let _ = ctx.ev_tx.send(BackendEvent::Feedback(feedback));
    }
}

/// Destroy every link between two nodes (used when the guard refuses a send).
fn remove_links_between(ctx: &Ctx, st: &mut WorkerState, out_node: NodeId, in_node: NodeId) {
    let ids: Vec<u32> = st
        .graph
        .links
        .values()
        .filter(|l| l.out_node == out_node && l.in_node == in_node)
        .map(|l| l.id)
        .collect();
    if !ids.is_empty() {
        log::info!("UNLINK {}  ->  {}", nname(st, out_node), nname(st, in_node));
    }
    for id in ids {
        st.graph.links.remove(&id);
        let _ = ctx.registry.destroy_global(id);
    }
}

/// Set a PipeWire default device via the "default" metadata object — the same
/// mechanism `wpctl set-default` uses, so it persists like any manual change.
fn set_default(ctx: &Ctx, key: &str, node_name: &str) -> bool {
    let st = ctx.st.borrow();
    let Some(md) = st.metadata.as_ref() else {
        drop(st);
        ctx.log("cannot set default: PipeWire metadata unavailable");
        return false;
    };
    let json = format!("{{\"name\":\"{node_name}\"}}");
    md.set_property(0, key, Some("Spa:String:JSON"), Some(&json));
    true
}

fn find_hw_sink(st: &WorkerState, target: Option<&str>) -> Option<NodeId> {
    let mut sinks: Vec<&registry::PwNode> = st
        .graph
        .nodes
        .values()
        .filter(|n| n.kind() == NodeKind::HwSink && !n.name.starts_with(registry::OUR_PREFIX))
        .collect();
    sinks.sort_by_key(|n| n.id);
    match target {
        Some(t) if !t.is_empty() => {
            let t = t.to_lowercase();
            sinks.iter().find(|n| n.name.to_lowercase().contains(&t) || n.label().to_lowercase().contains(&t)).map(|n| n.id)
        }
        _ => sinks.first().map(|n| n.id),
    }
}

/// Short human name for a graph node, for the log.
fn nname(st: &WorkerState, id: NodeId) -> String {
    st.graph
        .nodes
        .get(&id)
        .map(|n| format!("{} [{}]", n.label(), n.name))
        .unwrap_or_else(|| format!("node {id}"))
}

fn ensure_links(core: &pw::core::CoreRc, st: &mut WorkerState, slot: Slot, out_node: NodeId, in_node: NodeId) {
    let pairs = links::pair_ports(&st.graph, out_node, in_node);
    if pairs.is_empty() {
        // Dump what ports each side actually exposes — this is how we catch a
        // bus that presents no input ports (e.g. a virtual source whose sink
        // side isn't linkable), which otherwise fails silently.
        let outs: Vec<String> = st
            .graph
            .out_ports(out_node)
            .iter()
            .map(|p| format!("{}({:?})", p.id, p.channel))
            .collect();
        let ins: Vec<String> = st
            .graph
            .in_ports(in_node)
            .iter()
            .map(|p| format!("{}({:?})", p.id, p.channel))
            .collect();
        log::warn!(
            "NO PORT PAIR {} [out: {}] -> {} [in: {}] (will retry)",
            nname(st, out_node),
            outs.join(","),
            nname(st, in_node),
            ins.join(",")
        );
        return;
    }
    {
        let entry = st.link_proxies.entry(slot.clone()).or_default();
        entry.retain(|(p, _)| pairs.contains(p));
    }
    let mut made: Vec<((u32, u32), pw::link::Link)> = Vec::new();
    for p in pairs {
        let have = st.graph.link_exists(p.0, p.1)
            || st.link_proxies.get(&slot).map(|v| v.iter().any(|(q, _)| *q == p)).unwrap_or(false);
        if have {
            continue;
        }
        match links::create_link(core, out_node, p.0, in_node, p.1) {
            Ok(proxy) => made.push((p, proxy)),
            Err(e) => log::warn!("link FAILED {} -> {}: {e}", nname(st, out_node), nname(st, in_node)),
        }
    }
    if !made.is_empty() {
        log::info!("LINK  {}  ->  {}", nname(st, out_node), nname(st, in_node));
        st.link_proxies.entry(slot).or_default().extend(made);
    }
}

fn remove_slot_links(ctx: &Ctx, slot: &Slot) {
    let mut st = ctx.st.borrow_mut();
    let endpoints: Option<(Vec<NodeId>, NodeId)> = match slot {
        Slot::StripIn(idx) => {
            let key = st.desired.strip_input.get(idx).cloned();
            match (key, st.strip_nodes.get(idx).map(|(id, _)| *id)) {
                (Some(k), Some(n)) => Some((resolve_source(&st, &k), n)),
                (None, Some(n)) => {
                    // Input cleared: drop everything feeding the strip that
                    // isn't an app which pointed itself here.
                    let srcs: Vec<NodeId> = st
                        .graph
                        .links
                        .values()
                        .filter(|l| l.in_node == n)
                        .map(|l| l.out_node)
                        .filter(|o| {
                            st.graph.nodes.get(o).map(|nd| nd.kind()) == Some(NodeKind::HwSource)
                        })
                        .collect();
                    Some((srcs, n))
                }
                _ => None,
            }
        }
        Slot::Send(sidx, bidx) => {
            match (st.strip_nodes.get(sidx).map(|(id, _)| *id), st.bus_nodes.get(bidx).map(|(id, _)| *id)) {
                (Some(s), Some(b)) => Some((vec![s], b)),
                _ => None,
            }
        }
        Slot::Monitor(b, a) => {
            match (st.bus_nodes.get(b).map(|(id, _)| *id), st.bus_nodes.get(a).map(|(id, _)| *id)) {
                (Some(s), Some(d)) => Some((vec![s], d)),
                _ => None,
            }
        }
        Slot::BusListener(bus, cap) => {
            st.bus_nodes.get(bus).map(|(id, _)| *id).map(|bus_node| (vec![bus_node], *cap))
        }
        Slot::BusDev(_) => None,
    };
    if let Some((outs, in_node)) = endpoints {
        for o in outs {
            remove_links_between(ctx, &mut st, o, in_node);
        }
    }
    st.link_proxies.remove(slot);
}

fn handle_cmd(ctx: &Ctx, cmd: PwCmd) {
    match cmd {
        PwCmd::EnsureStrip { idx, label } => {
            ctx.st.borrow_mut().desired.strips.insert(idx, label);
            ensure_strip_device(ctx, idx);
            reconcile(ctx);
        }
        PwCmd::SetStripInput { idx, source_key } => {
            // No-op if nothing actually changed. The engine re-pushes strip
            // inputs whenever the app list moves, and blindly relinking caused
            // a needless UNLINK/LINK glitch on every app appearing.
            if ctx.st.borrow().desired.strip_input.get(&idx) == source_key.as_ref() {
                reconcile(ctx);
                return;
            }
            // Release any app we had pointed at this strip, so WirePlumber is
            // free to place it normally again.
            {
                let mut st = ctx.st.borrow_mut();
                if let Some(old) = st.desired.strip_input.get(&idx).cloned() {
                    let nodes = resolve_source(&st, &old);
                    for n in nodes {
                        if st.stream_targets.remove(&n).is_some() {
                            if let Some(md) = st.metadata.as_ref() {
                                md.set_property(n, "target.object", None, None);
                            }
                        }
                    }
                }
            }
            remove_slot_links(ctx, &Slot::StripIn(idx));
            {
                let mut st = ctx.st.borrow_mut();
                match source_key {
                    Some(k) => {
                        st.desired.strip_input.insert(idx, k);
                    }
                    None => {
                        st.desired.strip_input.remove(&idx);
                    }
                }
            }
            reconcile(ctx);
        }
        PwCmd::SetStripVolume { idx, volume } => {
            ctx.st.borrow_mut().desired.strip_vol.insert(idx, volume);
            apply_strip_controls(ctx);
        }
        PwCmd::SetStripMute { idx, mute } => {
            ctx.st.borrow_mut().desired.strip_mute.insert(idx, mute);
            // Mute also cuts/restores the strip's sends, so reconcile the graph
            // rather than only toggling the node flag.
            apply_strip_controls(ctx);
            reconcile(ctx);
        }
        PwCmd::SetStripAssign { idx, bus, on } => {
            if on {
                ctx.st.borrow_mut().desired.assigns.insert((idx, bus));
                reconcile(ctx);
            } else {
                ctx.st.borrow_mut().desired.assigns.remove(&(idx, bus));
                remove_slot_links(ctx, &Slot::Send(idx, bus));
                reconcile(ctx);
            }
        }
        PwCmd::SetFeedbackGuard { on } => {
            ctx.st.borrow_mut().feedback_guard = on;
            reconcile(ctx);
        }
        PwCmd::SetDefaultOutput { idx } => {
            let name = strip_name(idx);
            let ok = set_default(ctx, "default.audio.sink", &name);
            if ok {
                let _ = ctx.ev_tx.send(BackendEvent::DefaultOutput(Some(idx)));
                ctx.log(format!(
                    "system default output → {} (apps that follow the default now land here)",
                    m::strip_device_label(idx)
                ));
            }
        }
        PwCmd::SetDefaultInput { idx } => {
            let name = virtual_dev::bus_node_name(idx);
            let ok = set_default(ctx, "default.audio.source", &name);
            if ok {
                let _ = ctx.ev_tx.send(BackendEvent::DefaultInput(Some(idx)));
                ctx.log(format!("system default input → bus {}", idx + 1));
            }
        }
        PwCmd::EnsureBus { idx, label, kind } => {
            ctx.st.borrow_mut().desired.buses.insert(idx, (label.clone(), kind));
            ensure_bus_device(ctx, idx, &label, kind);
            reconcile(ctx);
        }
        PwCmd::SetBusDevice { idx, device } => {
            ctx.st.borrow_mut().desired.bus_dev.insert(idx, device);
            {
                let mut st = ctx.st.borrow_mut();
                st.link_proxies.remove(&Slot::BusDev(idx));
                if let Some(&(bus_node, _)) = st.bus_nodes.get(&idx) {
                    let ids: Vec<u32> = st
                        .graph
                        .links
                        .values()
                        .filter(|l| l.out_node == bus_node)
                        .map(|l| l.id)
                        .collect();
                    for id in ids {
                        st.graph.links.remove(&id);
                        let _ = ctx.registry.destroy_global(id);
                    }
                }
            }
            reconcile(ctx);
        }
        PwCmd::SetBusVolume { idx, volume } => {
            let mut st = ctx.st.borrow_mut();
            st.desired.bus_vol.insert(idx, volume);
            if let Some((_, node)) = st.bus_nodes.get(&idx) {
                let _ = virtual_dev::set_node_volume(node, volume);
            }
        }
        PwCmd::SetBusMute { idx, mute } => {
            {
                let mut st = ctx.st.borrow_mut();
                st.desired.bus_mute.insert(idx, mute);
                if let Some((_, node)) = st.bus_nodes.get(&idx) {
                    let _ = virtual_dev::set_node_mute(node, mute);
                }
            }
            // A B-bus is a null-sink source; the mute flag alone doesn't stop
            // its output reaching the app that captures it. Cut/restore the
            // actual links so MUTE truly silences the virtual mic — muting B2
            // must stop your voice reaching Discord.
            if mute {
                // Drop every listener + monitor link out of this bus.
                let caps: Vec<NodeId> = ctx
                    .st
                    .borrow()
                    .listener_links
                    .keys()
                    .filter(|(b, _)| *b == idx)
                    .map(|(_, c)| *c)
                    .collect();
                for cap in caps {
                    remove_slot_links(ctx, &Slot::BusListener(idx, cap));
                }
                let mons: Vec<usize> = ctx
                    .st
                    .borrow()
                    .monitors
                    .iter()
                    .filter(|(b, _)| *b == idx)
                    .map(|(_, a)| *a)
                    .collect();
                for a in mons {
                    remove_slot_links(ctx, &Slot::Monitor(idx, a));
                }
            } else {
                // Un-mute: reconcile rebuilds the wanted links.
                apply_bus_listeners(ctx);
                reconcile(ctx);
            }
        }
        PwCmd::SetBusMonitor { bus, a_bus, on } => {
            if on {
                ctx.st.borrow_mut().monitors.insert((bus, a_bus));
                reconcile(ctx);
            } else {
                ctx.st.borrow_mut().monitors.remove(&(bus, a_bus));
                remove_slot_links(ctx, &Slot::Monitor(bus, a_bus));
                reconcile(ctx);
            }
        }
        PwCmd::SetBusListener { bus, app_key } => {
            {
                let mut st = ctx.st.borrow_mut();
                // Release the previous app so WirePlumber can place it normally.
                if let Some(old) = st.bus_listener.remove(&bus) {
                    let nodes = resolve_capture(&st, &old);
                    for n in nodes {
                        if st.capture_targets.remove(&n).is_some() {
                            if let Some(md) = st.metadata.as_ref() {
                                md.set_property(n, "target.object", None, None);
                            }
                        }
                    }
                }
                if let Some(k) = app_key {
                    st.bus_listener.insert(bus, k);
                }
            }
            apply_bus_listeners(ctx);
            emit_listeners(ctx);
        }
        PwCmd::StartRecord { target, path } => {
            // Strips and A-buses are sinks (record their monitor); B-buses are
            // virtual sources (record their output directly).
            let (name, cap_sink) = {
                let st = ctx.st.borrow();
                match target {
                    RecTarget::Strip(i) => (strip_name(i), true),
                    RecTarget::Bus(i) => {
                        let kind =
                            st.desired.buses.get(&i).map(|(_, k)| *k).unwrap_or(BusKind::HwOutput);
                        (virtual_dev::bus_node_name(i), kind == BusKind::HwOutput)
                    }
                }
            };
            if let Some(mut old) = ctx.st.borrow_mut().recorders.remove(&target) {
                let _ = old.stop();
            }
            match recorder::Recorder::new(&ctx.core, &name, cap_sink, &path) {
                Ok(r) => {
                    ctx.st.borrow_mut().recorders.insert(target, r);
                    ctx.log(format!("recording {name} → {}", path.display()));
                }
                Err(e) => ctx.log(format!("record FAILED: {e}")),
            }
        }
        PwCmd::StopRecord { target } => {
            let rec = ctx.st.borrow_mut().recorders.remove(&target);
            if let Some(mut r) = rec {
                let _ = r.stop();
            }
            let _ = ctx.ev_tx.send(BackendEvent::RecordStopped(target));
        }
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;
    use crate::registry::{Graph, NodeId, PwLink, PwNode};

    fn app(id: u32, binary: &str, media_class: &str) -> PwNode {
        PwNode {
            id,
            name: format!("{binary}.{id}"),
            app_name: Some(binary.into()),
            binary: Some(binary.into()),
            description: None,
            media_class: media_class.into(),
        }
    }

    fn link(id: u32, out_node: NodeId, in_node: NodeId) -> PwLink {
        PwLink { id, out_node, in_node, out_port: 0, in_port: 0 }
    }

    /// Reproduces Havok's exact setup:
    ///   strip 0 = mic, strip 2 = Discord playback (WEBRTC),
    ///   Discord's capture (its mic) is listening to B2 (bus_node 500).
    /// Sending mic (strip 0) -> B2 must NOT be flagged as feedback.
    #[test]
    fn mic_to_b2_is_not_feedback_when_discord_listens_to_b2() {
        let mut g = Graph::default();
        // Discord capture stream, linked FROM B2 (bus 500) — Discord's mic = B2.
        let discord_cap = app(10, "discord", "Stream/Input/Audio");
        g.nodes.insert(10, discord_cap);
        g.links.insert(1, link(1, 500, 10)); // B2 -> Discord capture

        let mut strip_input = HashMap::new();
        strip_input.insert(0usize, "corsair".to_string()); // mic strip
        strip_input.insert(2usize, "discord".to_string()); // Discord playback strip

        // mic strip (0) -> B2 (500): NOT feedback.
        assert!(
            !listens(&g, &strip_input, 0, 500),
            "mic->B2 wrongly flagged as feedback"
        );
    }

    /// The guard MUST still fire for the real loop: Discord's own strip -> B2,
    /// when Discord captures B2 (it would hear itself).
    #[test]
    fn discord_strip_to_b2_is_feedback() {
        let mut g = Graph::default();
        g.nodes.insert(10, app(10, "discord", "Stream/Input/Audio"));
        g.links.insert(1, link(1, 500, 10)); // B2 -> Discord capture

        let mut strip_input = HashMap::new();
        strip_input.insert(2usize, "discord".to_string());

        assert!(
            listens(&g, &strip_input, 2, 500),
            "Discord->B2 must be blocked (it would hear itself)"
        );
    }

    /// The PHONE case: a SIP softphone whose capture reads B2 (bus 500) AND
    /// whose playback feeds the phone strip (node 700). Sending that strip into
    /// B2 must be caught as feedback even though the phone's configured key does
    /// NOT match its stream names (the name-based check alone would miss it).
    #[test]
    fn phone_self_loop_caught_structurally() {
        let mut g = Graph::default();
        // Phone capture reads B2.
        let mut pcap = app(30, "call-audio", "Stream/Input/Audio");
        pcap.binary = Some("call-audio-daemon".into()); // name != configured key
        g.nodes.insert(30, pcap);
        g.links.insert(1, link(1, 500, 30)); // B2 -> phone capture

        // Phone playback feeds the phone strip (node 700).
        let mut pplay = app(31, "call-audio", "Stream/Output/Audio");
        pplay.binary = Some("call-audio-daemon".into());
        g.nodes.insert(31, pplay);
        g.links.insert(2, link(2, 31, 700)); // phone playback -> strip

        // strip_input has a DIFFERENT key (mis-detected), so name-based fails...
        let mut strip_input = HashMap::new();
        strip_input.insert(1usize, "phone".to_string());

        // ...but structural check must still catch it.
        assert!(
            structural_feedback(&g, Some(700), 500),
            "phone self-loop must be caught by structural trace"
        );
    }

    /// Structural check must NOT false-positive: mic strip (700) sending to B2
    /// where only the PHONE (not mic) captures B2 is safe.
    #[test]
    fn structural_no_false_positive_for_mic() {
        let mut g = Graph::default();
        let pcap = app(30, "call-audio", "Stream/Input/Audio");
        g.nodes.insert(30, pcap);
        g.links.insert(1, link(1, 500, 30)); // B2 -> phone capture
        // Mic playback feeds the mic strip 700 (mic, not the phone).
        let mut mic = app(40, "mic-source", "Stream/Output/Audio");
        mic.binary = Some("pipewire".into());
        g.nodes.insert(40, mic);
        g.links.insert(2, link(2, 40, 700));
        assert!(
            !structural_feedback(&g, Some(700), 500),
            "mic->B2 must stay allowed even though phone captures B2"
        );
    }

    /// A phantom playback stream transiently linked to the mic strip must NOT
    /// make the mic strip an "owner" of Discord.
    #[test]
    fn phantom_playback_does_not_poison_owner() {
        let mut g = Graph::default();
        // Discord playback exists but mic strip's configured input is the mic.
        g.nodes.insert(20, app(20, "discord", "Stream/Output/Audio"));
        g.nodes.insert(10, app(10, "discord", "Stream/Input/Audio"));
        g.links.insert(1, link(1, 500, 10));

        let mut strip_input = HashMap::new();
        strip_input.insert(0usize, "corsair".to_string());

        let owners = owner_keys_of(&g, &strip_input, 0);
        assert_eq!(owners, vec!["corsair".to_string()], "mic strip owner must be only the mic");
        assert!(!listens(&g, &strip_input, 0, 500));
    }
}
