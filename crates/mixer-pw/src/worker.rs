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
use crate::{dsp, links, recorder, tap, virtual_dev, PwCmd};
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

/// If `name` is a DSP filter-chain node we created (`ferromix.dsp.{idx}.in` /
/// `.out`), return which strip it belongs to and whether it's the in or out
/// side. Used to keep these nodes out of the ordinary bus-adoption path (they
/// aren't adapter nodes we create via a factory — the filter-chain module
/// creates them itself once loaded) and to populate `dsp_nodes`.
fn dsp_node_role(name: &str) -> Option<(usize, bool)> {
    let rest = name.strip_prefix("ferromix.dsp.")?;
    let (idx_str, suffix) = rest.split_once('.')?;
    let idx: usize = idx_str.parse().ok()?;
    match suffix {
        "in" => Some((idx, true)),
        "out" => Some((idx, false)),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Slot {
    /// source -> strip device
    StripIn(usize),
    /// bus monitor -> A-bus (hear what the far end hears)
    Monitor(usize, usize),
    /// bus `from` monitor -> bus `to` input (bus-to-bus routing)
    Feed(usize, usize),
    /// bus monitor -> strip device (bus-to-strip routing, the reverse of
    /// `Send`) — lets a bus like B1 feed back into one or more strips.
    BusToStrip(usize, usize),
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
    /// Same as `BusListener`, for a strip acting as an app's mic feed —
    /// drawn manually for the same reason (belt-and-suspenders: works
    /// whether or not the strip's autoconnect setting would have let
    /// WirePlumber route it via metadata alone).
    StripListener(usize, NodeId),
    /// source -> strip's DSP filter-chain input (used instead of `StripIn`
    /// once a strip has a loaded gate/compressor module).
    DspIn(usize),
    /// strip's DSP filter-chain output -> strip device.
    DspOut(usize),
}

#[derive(Default)]
struct Desired {
    strips: HashMap<usize, String>,
    /// strip -> the source key linked into it (a mic or an app).
    strip_input: HashMap<usize, String>,
    strip_vol: HashMap<usize, f32>,
    strip_mute: HashMap<usize, bool>,
    /// strip -> force_mono (see `Strip.force_mono`'s doc comment).
    strip_force_mono: HashMap<usize, bool>,
    /// (strip, bus) sends.
    assigns: HashSet<(usize, usize)>,
    buses: HashMap<usize, (String, BusKind)>,
    bus_dev: HashMap<usize, Option<String>>,
    bus_vol: HashMap<usize, f32>,
    bus_mute: HashMap<usize, bool>,
    /// bus -> the source key directly linked into it (mirrors `strip_input`).
    bus_input: HashMap<usize, String>,
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
    /// (from, to) bus-to-bus feeds: `from`'s output additionally routed into
    /// `to`'s input.
    feeds: HashSet<(usize, usize)>,
    /// (bus, strip) bus-to-strip feeds: `bus`'s output additionally routed
    /// into `strip`'s device (the reverse of `Desired.assigns`).
    bus_strip_feeds: HashSet<(usize, usize)>,
    /// bus -> app key whose MICROPHONE we point at it.
    bus_listener: HashMap<usize, String>,
    /// Same as `bus_listener`, for a strip acting as an app's mic feed.
    strip_listener: HashMap<usize, String>,
    /// capture node -> bus/strip name we already pointed it at.
    capture_targets: HashMap<NodeId, String>,
    /// (bus idx, capture node) -> unit, tracking B-bus→app-mic links we drew
    /// ourselves so we can tear them down when the assignment changes.
    listener_links: HashMap<(usize, NodeId), ()>,
    /// Same as `listener_links`, for `strip_listener`.
    strip_listener_links: HashMap<(usize, NodeId), ()>,
    last_strip_listeners: Vec<(usize, Vec<String>)>,
    last_capture_apps: Vec<SourceInfo>,
    last_sources: Vec<SourceInfo>,
    last_devices: Vec<Device>,
    /// strip -> its loaded gate/compressor filter-chain module, if touched.
    dsp_modules: HashMap<usize, dsp::DspModule>,
    /// strip -> (dsp.in node, dsp.out node), filled in as each side's node
    /// appears in the registry after its module loads. Both must be `Some`
    /// before reconcile splices the DSP path in.
    dsp_nodes: HashMap<usize, (Option<NodeId>, Option<NodeId>)>,
    /// strip -> the raw `node.name` of the source its VU meter currently taps
    /// directly (pre-fader: the source's own output, not the strip's
    /// volume-controlled node — see `sync_prefader_tap`). Tracked so the tap
    /// is only torn down and recreated when the resolved source actually
    /// changes, not on every reconcile pass.
    strip_tap_src: HashMap<usize, String>,
    /// Same as `strip_tap_src`, for a bus's directly-assigned input (if any).
    /// A bus with no entry here (and no `desired.bus_input` entry) has NO
    /// tap at all — its meter is silent, never falling back to the bus's own
    /// (mixed/routed) node. See `sync_bus_prefader_tap`.
    bus_tap_src: HashMap<usize, String>,
}

#[derive(Clone)]
struct Ctx {
    // 0.9's `*Rc` handles are themselves reference-counted, so we no longer
    // wrap them in std::rc::Rc.
    core: pw::core::CoreRc,
    /// Needed only to load the per-strip DSP filter-chain module
    /// (`pw_context_load_module` operates on the context, not the core).
    context: pw::context::ContextRc,
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
        context,
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
            let dsp_role = dsp_node_role(&node.name);

            let ours = node.name.starts_with("ferromix.")
                && !node.name.contains(".tap.")
                && !node.name.contains(".rec.")
                && dsp_role.is_none();
            if let Some((idx, is_in)) = dsp_role {
                // Not an adapter node we created via a factory (the filter-chain
                // module creates these itself once loaded) — don't route it
                // through the bus-adoption path, just record which node id is
                // which side so reconcile can splice it into the strip's path.
                if let Ok(bound) = ctx.registry.bind::<pw::node::Node, _>(g) {
                    let mut st = ctx.st.borrow_mut();
                    st.bound.insert(node.id, bound);
                    let entry = st.dsp_nodes.entry(idx).or_insert((None, None));
                    if is_in {
                        entry.0 = Some(node.id);
                    } else {
                        entry.1 = Some(node.id);
                    }
                }
            } else if ours {
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
            reconcile_all(ctx);
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
                reconcile_all(ctx);
            }
        }
        pw::types::ObjectType::Link => {
            let Some(props) = g.props else { return };
            if let Some(link) = registry::parse_link(g.id, props) {
                ctx.st.borrow_mut().graph.links.insert(link.id, link);
                reconcile_all(ctx);
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
            if let Some((idx, is_in)) = dsp_node_role(&node.name) {
                // The module itself owns this node's lifetime; if it vanished
                // out from under us (e.g. module crashed), forget the id so
                // reconcile falls back to the direct source->strip path
                // instead of trying to link into a dead node.
                if let Some(entry) = st.dsp_nodes.get_mut(&idx) {
                    if is_in {
                        entry.0 = None;
                    } else {
                        entry.1 = None;
                    }
                }
            }
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
    reconcile_all(ctx);
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
                let _ = kind;
                // No tap here: a bus's meter is pre-fader and source-only,
                // driven entirely by its directly-assigned `input` (mirrors
                // strips — see `sync_bus_prefader_tap`). With no input
                // assigned the meter stays silent, even though real audio
                // may still be flowing through the bus from routed sends —
                // `reconcile()` step 1b is the sole owner of this tap.
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
            // No tap here: a strip's meter is pre-fader (see
            // `sync_prefader_tap`), which needs a resolved SOURCE node, not
            // this strip's own node — `reconcile()` step 1 sets it up (or
            // leaves it silent if there's no source yet) on the very next
            // pass, which always follows adoption.
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

/// Resolve a source key to the live node ids backing it. Pure over `Graph` so
/// it's unit-testable without a `WorkerState`/live PipeWire connection.
fn resolve_source(graph: &Graph, key: &str) -> Vec<NodeId> {
    let key_l = key.to_lowercase();
    graph
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

/// Point strip `idx`'s VU meter tap at its own resolved SOURCE node instead
/// of the strip's own volume-controlled node — genuinely pre-fader, since a
/// source is entirely upstream of the fader's `channelVolumes`
/// (`virtual_dev.rs`'s `monitor.channel-volumes = true` is what makes the
/// strip's OWN monitor follow the fader — tapping the source instead sits
/// before that entirely). This is also what makes the meter source-only: it
/// shows exactly what that one app/device is producing, never audio that
/// happens to arrive at the strip's OUTPUT via routing (there isn't any —
/// strips are pure destinations for playback, nothing plays back into them
/// except their own configured source).
///
/// `src` is the strip's first resolved input node this pass (`None` if it
/// has no live source right now, e.g. input cleared or the app closed) — the
/// tap is torn down and left absent in that case, so the meter reads silent
/// rather than showing stale/wrong levels. Re-attaches the tap only when the
/// resolved source's `node.name` actually changes, so this is cheap to call
/// on every reconcile pass (the common case is a no-op comparison).
fn sync_prefader_tap(ctx: &Ctx, st: &mut WorkerState, idx: usize, src: Option<NodeId>) {
    let want_name = src.and_then(|id| st.graph.nodes.get(&id)).map(|n| n.name.clone());
    if st.strip_tap_src.get(&idx) == want_name.as_ref() {
        return;
    }
    st.taps.remove(&LevelKey::Strip(idx));
    match &want_name {
        Some(name) => match tap::Tap::new(&ctx.core, LevelKey::Strip(idx), name, false, ctx.ev_tx.clone()) {
            Ok(t) => {
                st.taps.insert(LevelKey::Strip(idx), t);
                st.strip_tap_src.insert(idx, name.clone());
            }
            Err(e) => {
                log::warn!("pre-fader tap for strip {idx} on {name} FAILED: {e}");
                st.strip_tap_src.remove(&idx);
            }
        },
        None => {
            st.strip_tap_src.remove(&idx);
        }
    }
}

/// Same idea as `sync_prefader_tap`, for a bus's directly-assigned `input`.
/// Applies uniformly to A-buses and B-buses — a bus's meter reflects ONLY
/// this directly-assigned source, pre-fader, never the mixed content routed
/// in via the strip send matrix (`assign`) or bus-to-bus feeds (`feeds`).
/// `src` is `None` whenever the bus has no `input` assigned (or it hasn't
/// resolved to a live node yet) — the tap is simply absent in that case, so
/// the meter reads silent rather than falling back to any mixed content.
fn sync_bus_prefader_tap(ctx: &Ctx, st: &mut WorkerState, idx: usize, src: Option<NodeId>) {
    let want_name = src.and_then(|id| st.graph.nodes.get(&id)).map(|n| n.name.clone());
    if st.bus_tap_src.get(&idx) == want_name.as_ref() {
        return;
    }
    st.taps.remove(&LevelKey::Bus(idx));
    match &want_name {
        Some(name) => match tap::Tap::new(&ctx.core, LevelKey::Bus(idx), name, false, ctx.ev_tx.clone()) {
            Ok(t) => {
                st.taps.insert(LevelKey::Bus(idx), t);
                st.bus_tap_src.insert(idx, name.clone());
            }
            Err(e) => {
                log::warn!("pre-fader tap for bus {idx} on {name} FAILED: {e}");
                st.bus_tap_src.remove(&idx);
            }
        },
        None => {
            st.bus_tap_src.remove(&idx);
        }
    }
}

/// Faders and mutes act on the strip's own device node — never on the app or
/// the mic, so they behave identically for every kind of input.
/// Capture (microphone) stream nodes belonging to an app. Pure over `Graph`,
/// same reasoning as `resolve_source`.
fn resolve_capture(graph: &Graph, key: &str) -> Vec<NodeId> {
    let key_l = key.to_lowercase();
    graph
        .nodes
        .values()
        .filter(|n| {
            n.kind() == NodeKind::AppCapture && {
                let k = n.source_key();
                // Same substring fallback as resolve_source: a bus_listener key
                // recorded before an app's exact stream name settles (or that
                // drifts slightly across relaunches) would otherwise silently
                // match nothing, leaving the B-bus listener link never drawn.
                k == key_l || k.contains(&key_l)
            }
        })
        .map(|n| n.id)
        .collect()
}

/// Which nodes does `out_node`'s output currently reach besides anything in
/// `keep`? Used to redirect an app fully onto its assigned strip(s) instead
/// of also leaving it fanned out to wherever it was linked before (commonly a
/// pipewire-pulse role loopback sink, e.g. `input.loopback.sink.role.
/// multimedia` — confirmed live: Spotify was linked to BOTH its FerroMix
/// strip and that loopback simultaneously, so the strip's fader/mute had no
/// audible effect since Spotify kept playing through the untouched path).
///
/// `keep` is a slice, not a single node: an app CAN legitimately be assigned
/// to more than one FerroMix strip/bus at once (both are "ours" and desired),
/// and treating a second legitimate FerroMix destination as "stray" relative
/// to the first would make our own two desired links fight each other every
/// reconcile pass — confirmed live: with the same app assigned as both B1's
/// and B2's listener, an earlier single-`keep` version of this cut Discord's
/// B2 link "stray" while processing B1's, then cut B1's link "stray" while
/// processing B2's, back and forth every pass.
///
/// Pure over `Graph`, deduplicated (a stereo app has two links — FL/FR — to
/// the same stray destination, which should be one redirect, not two).
fn stray_destinations(graph: &Graph, out_node: NodeId, keep: &[NodeId]) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = graph
        .links
        .values()
        .filter(|l| l.out_node == out_node && !keep.contains(&l.in_node))
        .map(|l| l.in_node)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Which nodes currently feed INTO `in_node` besides anything in `keep`? The
/// capture-side mirror of `stray_destinations` (see its doc for why `keep` is
/// a slice, not a single node — an app can legitimately listen to more than
/// one FerroMix bus at once). An app's microphone capture node commonly has
/// its real default microphone auto-connected by WirePlumber the instant the
/// node appears (`find-default-target.lua` runs synchronously, before our own
/// B-bus link can land), so pointing it at a B-bus without cutting that stray
/// source leaves the app hearing its real mic and the bus mixed together
/// permanently — the same class of bug `stray_destinations` fixes for
/// playback, just the opposite link direction.
fn stray_sources(graph: &Graph, in_node: NodeId, keep: &[NodeId]) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = graph
        .links
        .values()
        .filter(|l| l.in_node == in_node && !keep.contains(&l.out_node))
        .map(|l| l.out_node)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Point each assigned app's microphone at whichever B-bus AND/OR strip it's
/// been given to, using the same `target.object` metadata trick we use for
/// playback. This is what lets you set Discord's mic to B1 (or to a strip)
/// from inside FerroMix instead of digging through Discord's own settings.
///
/// Buses and strips are handled together, sharing one stray-redirect pass,
/// because an app can legitimately listen to a bus AND a strip at once (same
/// reasoning as multiple buses at once) — computing the "what's legitimately
/// feeding this capture node" set separately per bus/strip would make each
/// pass treat the OTHER's link as stray and fight it, the exact
/// self-fighting bug class `stray_sources`'s own doc comment warns about.
fn apply_listeners(ctx: &Ctx) {
    // Draw bus/strip -> app-capture links ourselves. WirePlumber won't route
    // into a node marked node.autoconnect=false, so target.object silently
    // failed and the app's mic fell back to the default source (the
    // Spotify+mic bleed) — and we draw strip links the same way for
    // consistency/reliability even though strips don't disable autoconnect.
    let desired_bus: Vec<(usize, NodeId, NodeId)> = {
        let st = ctx.st.borrow();
        let mut v = Vec::new();
        for (bus, key) in st.bus_listener.iter() {
            // A muted B-bus sends nothing — skip its listener link entirely.
            if st.desired.bus_mute.get(bus).copied().unwrap_or(false) {
                continue;
            }
            let Some(&(bus_node, _)) = st.bus_nodes.get(bus) else { continue };
            for cap in resolve_capture(&st.graph, key) {
                v.push((*bus, bus_node, cap));
            }
        }
        v
    };
    let desired_strip: Vec<(usize, NodeId, NodeId)> = {
        let st = ctx.st.borrow();
        let mut v = Vec::new();
        for (strip, key) in st.strip_listener.iter() {
            // Same reasoning as the bus case: a muted strip sends nothing.
            if st.desired.strip_mute.get(strip).copied().unwrap_or(false) {
                continue;
            }
            let Some(&(strip_node, _)) = st.strip_nodes.get(strip) else { continue };
            for cap in resolve_capture(&st.graph, key) {
                v.push((*strip, strip_node, cap));
            }
        }
        v
    };

    // Remove any stale listener links whose app is no longer assigned.
    let stale_bus: Vec<(usize, NodeId)> = {
        let st = ctx.st.borrow();
        st.listener_links
            .keys()
            .copied()
            .filter(|(bus, cap)| !desired_bus.iter().any(|(b, _, c)| b == bus && c == cap))
            .collect()
    };
    for (bus, cap) in stale_bus {
        remove_slot_links(ctx, &Slot::BusListener(bus, cap));
        ctx.st.borrow_mut().listener_links.remove(&(bus, cap));
    }
    let stale_strip: Vec<(usize, NodeId)> = {
        let st = ctx.st.borrow();
        st.strip_listener_links
            .keys()
            .copied()
            .filter(|(s, cap)| !desired_strip.iter().any(|(sx, _, c)| sx == s && c == cap))
            .collect()
    };
    for (strip, cap) in stale_strip {
        remove_slot_links(ctx, &Slot::StripListener(strip, cap));
        ctx.st.borrow_mut().strip_listener_links.remove(&(strip, cap));
    }

    // Ensure the wanted links exist.
    for &(bus, bus_node, cap) in &desired_bus {
        let already = ctx.st.borrow().listener_links.contains_key(&(bus, cap));
        if !already {
            let mut st = ctx.st.borrow_mut();
            ensure_links(&ctx.core, &mut st, Slot::BusListener(bus, cap), bus_node, cap);
            st.listener_links.insert((bus, cap), ());
            log::info!("MIC LINK bus.{bus} -> {}", nname(&st, cap));
        }
    }
    for &(strip, strip_node, cap) in &desired_strip {
        let already = ctx.st.borrow().strip_listener_links.contains_key(&(strip, cap));
        if !already {
            let mut st = ctx.st.borrow_mut();
            ensure_links(&ctx.core, &mut st, Slot::StripListener(strip, cap), strip_node, cap);
            st.strip_listener_links.insert((strip, cap), ());
            log::info!("MIC LINK strip.{strip} -> {}", nname(&st, cap));
        }
    }

    // Redirect: cut anything feeding an app's capture that ISN'T one of its
    // legitimately-assigned FerroMix buses/strips — typically the app's real
    // default microphone, auto-connected by WirePlumber before we could
    // react (see `stray_sources`'s doc). Grouped by capture node first,
    // keeping EVERY bus/strip assigned to it (an app can legitimately listen
    // to more than one at once). Runs every pass, not gated behind
    // `already`, so a stray link that reappears later still gets caught —
    // cheap no-op once the capture is already exclusive to FerroMix.
    let mut legit_by_cap: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for &(_, node, cap) in desired_bus.iter().chain(desired_strip.iter()) {
        legit_by_cap.entry(cap).or_default().push(node);
    }
    for (cap, keep) in legit_by_cap {
        let stray = {
            let st = ctx.st.borrow();
            stray_sources(&st.graph, cap, &keep)
        };
        for other in stray {
            let mut st = ctx.st.borrow_mut();
            log::info!("REDIRECT {} off {} (mic now exclusively FerroMix)", nname(&st, cap), nname(&st, other));
            remove_links_between(ctx, &mut st, other, cap);
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
    let strip_listeners: Vec<(usize, Vec<String>)> = {
        let st = ctx.st.borrow();
        let mut out = Vec::new();
        for (idx, (node, _)) in st.strip_nodes.iter() {
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
    if strip_listeners != st.last_strip_listeners {
        st.last_strip_listeners = strip_listeners.clone();
        let _ = ctx.ev_tx.send(BackendEvent::StripListeners(strip_listeners));
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

/// Converge the graph AND the B-bus→app-mic listener links in one pass. Every
/// call site that used to call `reconcile()` alone should call this instead —
/// `apply_bus_listeners` is desired-state convergence exactly like the rest of
/// `reconcile()`, just factored out because it needs its own borrow. Before
/// this, a listener link that got torn down by node churn (e.g. an app's
/// capture stream briefly disappearing on reconnect) only got redrawn if a
/// command handler happened to call `apply_bus_listeners` explicitly —
/// `on_global_remove` never did, so a dropped-and-recreated capture node could
/// permanently strand a bus's mix-minus feed until the user re-picked the app
/// in SEND TO APP. Folding it in here means every registry event and every
/// command reconverges listener links too, matching the "declarative
/// reconciler" architecture (see docs/ARCHITECTURE.md).
fn reconcile_all(ctx: &Ctx) {
    apply_listeners(ctx);
    reconcile(ctx);
}

fn reconcile(ctx: &Ctx) {
    let mut feedback: Vec<(usize, usize)> = Vec::new();
    {
        let mut st = ctx.st.borrow_mut();
        let st = &mut *st;

        // 1. source -> strip device. If the strip has a loaded gate/compressor
        //    (both its dsp.in and dsp.out nodes have shown up in the registry),
        //    route source -> dsp.in and dsp.out -> strip device instead of a
        //    direct link, so the chain sits between the source and everything
        //    the fader/mute/meter/sends act on. Strips that have never touched
        //    DSP take the direct path exactly as before — zero overhead.
        let inputs: Vec<(usize, String)> =
            st.desired.strip_input.iter().map(|(i, k)| (*i, k.clone())).collect();
        for (idx, key) in inputs {
            let Some(&(strip_node, _)) = st.strip_nodes.get(&idx) else { continue };
            let srcs = resolve_source(&st.graph, &key);
            let force_mono = st.desired.strip_force_mono.get(&idx).copied().unwrap_or(false);
            let dsp_target = st.dsp_nodes.get(&idx).and_then(|(i, o)| i.zip(*o));
            match dsp_target {
                Some((dsp_in, dsp_out)) => {
                    // `dsp_target` goes `Some` as soon as the DSP module's
                    // NODE objects appear in the registry — which happens
                    // strictly before their PORTS do (separate async
                    // registry events). Previously this unconditionally cut
                    // the working direct source→strip link right here,
                    // before there was any confirmation the replacement
                    // DspIn/DspOut links could actually form. Normally the
                    // race is invisible (ports show up moments later in the
                    // same sync burst and reconcile re-runs) — but if the
                    // filter-chain module's internal gate+compressor graph
                    // ever fails to fully negotiate ports (any
                    // environment-specific LADSPA/module hiccup), the direct
                    // link was already gone and the DSP link would never
                    // form: permanent, total silence on that strip with no
                    // way back short of restarting FerroMix. Fixed by
                    // attempting the DSP links FIRST, and only cutting the
                    // direct link once DspOut is CONFIRMED actually linked
                    // in the live graph (not just "we asked for it") — worst
                    // case during the handoff window you briefly hear both
                    // paths, never silence, and a permanently-stuck module
                    // just leaves you on the unprocessed direct signal
                    // forever instead of killing the strip.
                    for &src in &srcs {
                        if force_mono {
                            ensure_links_forced_mono(&ctx.core, st, Slot::DspIn(idx), src, dsp_in);
                        } else {
                            ensure_links(&ctx.core, st, Slot::DspIn(idx), src, dsp_in);
                        }
                    }
                    ensure_links(&ctx.core, st, Slot::DspOut(idx), dsp_out, strip_node);
                    let dsp_confirmed = st
                        .graph
                        .links
                        .values()
                        .any(|l| l.out_node == dsp_out && l.in_node == strip_node);
                    if dsp_confirmed {
                        for &src in &srcs {
                            remove_links_between(ctx, st, src, strip_node);
                        }
                        st.link_proxies.remove(&Slot::StripIn(idx));
                    }
                }
                None => {
                    for &src in &srcs {
                        if force_mono {
                            ensure_links_forced_mono(&ctx.core, st, Slot::StripIn(idx), src, strip_node);
                        } else {
                            ensure_links(&ctx.core, st, Slot::StripIn(idx), src, strip_node);
                        }
                    }
                }
            }
            sync_prefader_tap(ctx, st, idx, srcs.first().copied());
        }

        // 1b. bus direct input: METERING ONLY, deliberately no routing link.
        //     Unlike a strip (whose whole job is to carry its source's real
        //     audio through the mix), a bus's direct input exists purely so
        //     its meter can show that one source's level — actually LINKING
        //     the source into the bus's sink (like step 1 does for strips)
        //     would physically mix that audio into the bus, which is
        //     catastrophic when the bus is also a SEND TO APP target for the
        //     same app: e.g. Discord's own incoming voice would get routed
        //     back into the bus Discord captures as its mic, so Discord's own
        //     echo canceller suppresses the real mic signal it's tangled up
        //     with. `sync_bus_prefader_tap` uses a separate, non-invasive
        //     capture stream (a `tap::Tap`, same as strips' pre-fader tap)
        //     that observes the source directly without touching the graph.
        let bus_inputs: Vec<(usize, String)> =
            st.desired.bus_input.iter().map(|(i, k)| (*i, k.clone())).collect();
        for (idx, key) in bus_inputs {
            if !st.bus_nodes.contains_key(&idx) {
                continue;
            }
            let srcs = resolve_source(&st.graph, &key);
            sync_bus_prefader_tap(ctx, st, idx, srcs.first().copied());
        }

        // 2. Point each app that belongs on a strip AT that strip's device,
        //    using PipeWire metadata (`target.object`) — the same mechanism
        //    `pw-metadata <id> target.object <name>` uses.
        //
        //    We must NOT repeatedly destroy the app's links every reconcile
        //    pass: WirePlumber owns stream placement and would instantly
        //    recreate them, turning into an endless destroy/recreate war (it
        //    did — hundreds of times a second). Setting the target tells the
        //    session manager where the stream belongs, and it moves it and
        //    keeps it there for links it creates AFTER this point.
        //
        //    But metadata alone isn't enough for pipewire-pulse clients
        //    (Spotify, Firefox, anything going through the PulseAudio
        //    compat layer): confirmed live that Spotify's stream was fanned
        //    out to BOTH its FerroMix strip AND
        //    input.loopback.sink.role.multimedia (a pipewire-pulse role-based
        //    loopback sink) simultaneously — the role loopback link is
        //    created by pipewire-pulse's own routing, independent of (and not
        //    reverted by) target.object. That left the strip's fader/mute
        //    with no audible effect, since the app kept playing through the
        //    untouched loopback path. Confirmed empirically that cutting that
        //    stray link is a ONE-TIME fix, not a fight: it does not get
        //    recreated afterward, so we do it exactly once per (app, strip)
        //    assignment — the same instant we set the metadata — never on
        //    every pass, which is what avoids re-triggering the documented war.
        let targets: Vec<(NodeId, NodeId, String)> = {
            let mut v = Vec::new();
            for (idx, key) in st.desired.strip_input.iter() {
                let Some(&(strip_node, _)) = st.strip_nodes.get(idx) else { continue };
                let name = strip_name(*idx);
                for src in resolve_source(&st.graph, key) {
                    if st.graph.nodes.get(&src).map(|n| n.kind()) == Some(NodeKind::AppPlayback) {
                        v.push((src, strip_node, name.clone()));
                    }
                }
            }
            v
        };
        for (node, _strip_node, target) in &targets {
            // Metadata write is still latched to once-per-(node,target): this
            // is the part that risked the destroy/recreate war if repeated.
            if st.stream_targets.get(node) != Some(target) {
                st.stream_targets.insert(*node, target.clone());
                if let Some(md) = st.metadata.as_ref() {
                    let json = format!("\"{target}\"");
                    md.set_property(*node, "target.object", Some("Spa:String:JSON"), Some(&json));
                    log::info!("TARGET {} -> {}", nname(st, *node), target);
                } else {
                    log::warn!("cannot retarget {}: no metadata object", nname(st, *node));
                }
            }
        }
        // Redirect: cut anything else this app's output reaches besides its
        // legitimately-assigned strip(s). Grouped by source node, keeping
        // EVERY strip it's assigned to — an app can legitimately be assigned
        // as the input for more than one strip, and treating a second
        // legitimate strip as "stray" relative to the first would make our
        // own two desired links fight each other every pass (confirmed live
        // for the capture-side equivalent of this, see `stray_sources`'s
        // doc — same fix applied here for consistency). Deliberately NOT
        // gated behind the metadata latch above — cheap no-op once already
        // exclusive, but catches a stray link that reappears later without
        // the node itself disappearing, which the old one-shot-only version
        // could never see again once `stream_targets` already matched.
        let mut legit_by_src: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for &(node, strip_node, _) in &targets {
            legit_by_src.entry(node).or_default().push(strip_node);
        }
        for (node, keep) in legit_by_src {
            for other in stray_destinations(&st.graph, node, &keep) {
                log::info!("REDIRECT {} off {} (now exclusively on FerroMix)", nname(st, node), nname(st, other));
                remove_links_between(ctx, st, node, other);
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

        // 3b. Bus monitoring: hear what the app assigned to a B-bus (its
        //     `listener`) is actually SENDING you — that app's own playback,
        //     e.g. Discord's incoming voice — NOT the B-bus's own mix (which
        //     is what you're sending THEM, typically including your own
        //     mic). Those are two independent directions sharing the same
        //     bus "stack" for convenience: mic → B2 → Discord's capture (the
        //     `assign`/listener machinery, unchanged) sends your voice out;
        //     this step pipes Discord's own voice back to your speakers,
        //     with no shared audio between the two paths, so you never hear
        //     yourself. No listener assigned = nothing to monitor.
        let mons: Vec<(usize, usize)> = st.monitors.iter().copied().collect();
        for (b, a) in mons {
            let Some(&(dst, _)) = st.bus_nodes.get(&a) else { continue };
            let Some(key) = st.bus_listener.get(&b).cloned() else { continue };
            for src in resolve_source(&st.graph, &key) {
                ensure_links(&ctx.core, st, Slot::Monitor(b, a), src, dst);
            }
        }

        // 3c. Bus-to-bus feeds: a bus's output additionally routed into
        //     another bus's input, alongside whatever strips send to it.
        let fds: Vec<(usize, usize)> = st.feeds.iter().copied().collect();
        for (from, to) in fds {
            let (Some(&(src, _)), Some(&(dst, _))) = (st.bus_nodes.get(&from), st.bus_nodes.get(&to))
            else {
                continue;
            };
            ensure_links(&ctx.core, st, Slot::Feed(from, to), src, dst);
        }

        // 3d. Bus-to-strip feeds: a bus's output additionally routed into a
        //     strip's device, the reverse of step 3's strip→bus sends. A
        //     strip's meter stays pre-fader/source-only regardless (see
        //     `sync_prefader_tap`), so this never makes a strip's meter move
        //     — only its own assigned `input` does.
        let bsf: Vec<(usize, usize)> = st.bus_strip_feeds.iter().copied().collect();
        for (bus, strip) in bsf {
            let (Some(&(src, _)), Some(&(dst, _))) = (st.bus_nodes.get(&bus), st.strip_nodes.get(&strip))
            else {
                continue;
            };
            ensure_links(&ctx.core, st, Slot::BusToStrip(bus, strip), src, dst);
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
            // MUTE means the bus sends nowhere. Same reasoning as strip mute
            // (step 3): a null-sink's mute flag alone doesn't stop its
            // monitor from emitting, so cut the actual link rather than
            // relying on `set_node_mute`. This guard was missing entirely
            // before — muting an A-bus never actually silenced it, the
            // hardware-output link stayed up regardless of mute state.
            // Sweep ALL of bus_node's current links rather than only the
            // configured target, so a stale link to a since-changed device
            // still gets cut.
            if st.desired.bus_mute.get(&b).copied().unwrap_or(false) {
                let mut dests: Vec<NodeId> =
                    st.graph.links.values().filter(|l| l.out_node == bus_node).map(|l| l.in_node).collect();
                dests.sort();
                dests.dedup();
                for hw in dests {
                    remove_links_between(ctx, st, bus_node, hw);
                }
                continue;
            }
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
    ensure_links_with_pairs(core, st, slot, out_node, in_node, pairs);
}

/// Same as `ensure_links`, for a strip input that has `force_mono` on —
/// forces the source's first port into every destination channel evenly
/// (see `links::pair_ports_forced_mono`'s doc comment) instead of the normal
/// channel-matched/topology-based pairing.
fn ensure_links_forced_mono(core: &pw::core::CoreRc, st: &mut WorkerState, slot: Slot, out_node: NodeId, in_node: NodeId) {
    let pairs = links::pair_ports_forced_mono(&st.graph, out_node, in_node);
    ensure_links_with_pairs(core, st, slot, out_node, in_node, pairs);
}

fn ensure_links_with_pairs(
    core: &pw::core::CoreRc,
    st: &mut WorkerState,
    slot: Slot,
    out_node: NodeId,
    in_node: NodeId,
    pairs: Vec<(u32, u32)>,
) {
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
                (Some(k), Some(n)) => Some((resolve_source(&st.graph, &k), n)),
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
            // Mirrors the new source resolution in reconcile()'s step 3b:
            // the link's real source is the bus's listener app's playback,
            // not the bus's own node — see that step's comment for why.
            match (st.bus_listener.get(b).cloned(), st.bus_nodes.get(a).map(|(id, _)| *id)) {
                (Some(k), Some(d)) => Some((resolve_source(&st.graph, &k), d)),
                _ => None,
            }
        }
        Slot::Feed(from, to) => {
            match (st.bus_nodes.get(from).map(|(id, _)| *id), st.bus_nodes.get(to).map(|(id, _)| *id)) {
                (Some(s), Some(d)) => Some((vec![s], d)),
                _ => None,
            }
        }
        Slot::BusToStrip(bus, strip) => {
            match (st.bus_nodes.get(bus).map(|(id, _)| *id), st.strip_nodes.get(strip).map(|(id, _)| *id)) {
                (Some(s), Some(d)) => Some((vec![s], d)),
                _ => None,
            }
        }
        Slot::BusListener(bus, cap) => {
            st.bus_nodes.get(bus).map(|(id, _)| *id).map(|bus_node| (vec![bus_node], *cap))
        }
        Slot::StripListener(strip, cap) => {
            st.strip_nodes.get(strip).map(|(id, _)| *id).map(|strip_node| (vec![strip_node], *cap))
        }
        Slot::BusDev(_) => None,
        Slot::DspIn(idx) => {
            let key = st.desired.strip_input.get(idx).cloned();
            let dsp_in = st.dsp_nodes.get(idx).and_then(|(i, _)| *i);
            match (key, dsp_in) {
                (Some(k), Some(dsp_in)) => Some((resolve_source(&st.graph, &k), dsp_in)),
                _ => None,
            }
        }
        Slot::DspOut(idx) => {
            let dsp_out = st.dsp_nodes.get(idx).and_then(|(_, o)| *o);
            let strip_node = st.strip_nodes.get(idx).map(|(id, _)| *id);
            match (dsp_out, strip_node) {
                (Some(dsp_out), Some(strip_node)) => Some((vec![dsp_out], strip_node)),
                _ => None,
            }
        }
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
            reconcile_all(ctx);
        }
        PwCmd::SetStripInput { idx, source_key } => {
            // No-op if nothing actually changed. The engine re-pushes strip
            // inputs whenever the app list moves, and blindly relinking caused
            // a needless UNLINK/LINK glitch on every app appearing.
            if ctx.st.borrow().desired.strip_input.get(&idx) == source_key.as_ref() {
                reconcile_all(ctx);
                return;
            }
            // Release any app we had pointed at this strip, so WirePlumber is
            // free to place it normally again.
            {
                let mut st = ctx.st.borrow_mut();
                if let Some(old) = st.desired.strip_input.get(&idx).cloned() {
                    let nodes = resolve_source(&st.graph, &old);
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
                        // Reconcile step 1 only re-evaluates the tap for
                        // strips still present in `strip_input` — clearing
                        // the key removes this strip from that loop entirely,
                        // so the pre-fader tap must be torn down explicitly
                        // here or it would keep showing the old source
                        // forever instead of going silent.
                        st.taps.remove(&LevelKey::Strip(idx));
                        st.strip_tap_src.remove(&idx);
                    }
                }
            }
            reconcile_all(ctx);
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
            reconcile_all(ctx);
        }
        PwCmd::SetStripAssign { idx, bus, on } => {
            if on {
                ctx.st.borrow_mut().desired.assigns.insert((idx, bus));
                reconcile_all(ctx);
            } else {
                ctx.st.borrow_mut().desired.assigns.remove(&(idx, bus));
                remove_slot_links(ctx, &Slot::Send(idx, bus));
                reconcile_all(ctx);
            }
        }
        PwCmd::SetStripDsp { idx, dsp } => {
            // Always (re)load: a fresh module with the new control values
            // baked into its SPA-JSON args. Replacing the HashMap entry drops
            // (and so destroys, via DspModule's Drop) whatever module was
            // there before — see dsp.rs's docstring for why this reloads
            // instead of pushing live param updates into the running chain.
            match dsp::load_filter_chain(&ctx.context, idx, &dsp) {
                Ok(module) => {
                    let mut st = ctx.st.borrow_mut();
                    st.dsp_modules.insert(idx, module);
                    // The old dsp.in/out node ids (if any) just got destroyed
                    // along with the old module — forget them so reconcile
                    // falls back to the direct source->strip path until the
                    // new module's nodes show up in the registry.
                    st.dsp_nodes.remove(&idx);
                    drop(st);
                    ctx.log(format!(
                        "DSP strip {} — gate {} / comp {}",
                        idx + 1,
                        if dsp.gate_on { "on" } else { "off" },
                        if dsp.comp_on { "on" } else { "off" }
                    ));
                    reconcile_all(ctx);
                }
                Err(e) => ctx.log(format!("DSP load FAILED for strip {}: {e}", idx + 1)),
            }
        }
        PwCmd::SetStripForceMono { idx, on } => {
            // The pairing scheme itself changes (normal vs. forced-mono), so
            // tear down whatever's currently linked before flipping the flag
            // — `ensure_links` only ADDS missing pairs, it won't notice the
            // desired pairing changed under an unchanged Slot key.
            remove_slot_links(ctx, &Slot::StripIn(idx));
            remove_slot_links(ctx, &Slot::DspIn(idx));
            ctx.st.borrow_mut().desired.strip_force_mono.insert(idx, on);
            reconcile_all(ctx);
        }
        PwCmd::SetFeedbackGuard { on } => {
            ctx.st.borrow_mut().feedback_guard = on;
            reconcile_all(ctx);
        }
        PwCmd::EnsureBus { idx, label, kind } => {
            ctx.st.borrow_mut().desired.buses.insert(idx, (label.clone(), kind));
            ensure_bus_device(ctx, idx, &label, kind);
            reconcile_all(ctx);
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
            reconcile_all(ctx);
        }
        PwCmd::SetBusInput { idx, source_key } => {
            // Same no-op guard as SetStripInput, same reasoning.
            if ctx.st.borrow().desired.bus_input.get(&idx) == source_key.as_ref() {
                reconcile_all(ctx);
                return;
            }
            // Metering-only (see the big comment in reconcile()'s step 1b):
            // there's no routing link to tear down here, just the tap, which
            // reconcile handles via `bus_tap_src` comparison / the explicit
            // clear below.
            {
                let mut st = ctx.st.borrow_mut();
                match source_key {
                    Some(k) => {
                        st.desired.bus_input.insert(idx, k);
                    }
                    None => {
                        st.desired.bus_input.remove(&idx);
                        // See the matching comment in SetStripInput: reconcile
                        // step 1b only re-evaluates the tap for buses still
                        // present in `bus_input`, so clear it explicitly here.
                        st.taps.remove(&LevelKey::Bus(idx));
                        st.bus_tap_src.remove(&idx);
                    }
                }
            }
            reconcile_all(ctx);
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
                // Same for bus-to-bus feeds this bus sends out (outgoing
                // direction only — a bus that's fed BY this one has its own
                // mute check independently; nothing extra needed here).
                let feeds_out: Vec<usize> = ctx
                    .st
                    .borrow()
                    .feeds
                    .iter()
                    .filter(|(from, _)| *from == idx)
                    .map(|(_, to)| *to)
                    .collect();
                for to in feeds_out {
                    remove_slot_links(ctx, &Slot::Feed(idx, to));
                }
                // Same for bus-to-strip feeds this bus sends out.
                let strip_feeds_out: Vec<usize> = ctx
                    .st
                    .borrow()
                    .bus_strip_feeds
                    .iter()
                    .filter(|(bus, _)| *bus == idx)
                    .map(|(_, strip)| *strip)
                    .collect();
                for strip in strip_feeds_out {
                    remove_slot_links(ctx, &Slot::BusToStrip(idx, strip));
                }
                // A-bus (hardware output): drop its hardware-sink link too.
                // This was missing entirely before — muting an A-bus never
                // actually silenced it, since only listener/monitor links
                // were cut here and reconcile()'s step 4 never checked mute
                // at all (now fixed there too, but that only takes effect on
                // the next reconcile pass — cutting it here makes MUTE take
                // effect immediately, same as the listener/monitor cuts above).
                let bus_node = ctx.st.borrow().bus_nodes.get(&idx).map(|(id, _)| *id);
                if let Some(bus_node) = bus_node {
                    let dests: Vec<NodeId> = {
                        let st = ctx.st.borrow();
                        let mut d: Vec<NodeId> =
                            st.graph.links.values().filter(|l| l.out_node == bus_node).map(|l| l.in_node).collect();
                        d.sort();
                        d.dedup();
                        d
                    };
                    let mut st = ctx.st.borrow_mut();
                    for hw in dests {
                        remove_links_between(ctx, &mut st, bus_node, hw);
                    }
                }
            } else {
                // Un-mute: reconcile_all rebuilds the wanted links (including
                // the listener link and, now, the hardware-device link).
                reconcile_all(ctx);
            }
        }
        PwCmd::SetBusMonitor { bus, a_bus, on } => {
            if on {
                ctx.st.borrow_mut().monitors.insert((bus, a_bus));
                reconcile_all(ctx);
            } else {
                ctx.st.borrow_mut().monitors.remove(&(bus, a_bus));
                remove_slot_links(ctx, &Slot::Monitor(bus, a_bus));
                reconcile_all(ctx);
            }
        }
        PwCmd::SetBusFeed { from, to, on } => {
            if on {
                ctx.st.borrow_mut().feeds.insert((from, to));
                reconcile_all(ctx);
            } else {
                ctx.st.borrow_mut().feeds.remove(&(from, to));
                remove_slot_links(ctx, &Slot::Feed(from, to));
                reconcile_all(ctx);
            }
        }
        PwCmd::SetBusStripFeed { bus, strip, on } => {
            if on {
                ctx.st.borrow_mut().bus_strip_feeds.insert((bus, strip));
                reconcile_all(ctx);
            } else {
                ctx.st.borrow_mut().bus_strip_feeds.remove(&(bus, strip));
                remove_slot_links(ctx, &Slot::BusToStrip(bus, strip));
                reconcile_all(ctx);
            }
        }
        PwCmd::SetBusListener { bus, app_key } => {
            let old_key = ctx.st.borrow().bus_listener.get(&bus).cloned();
            {
                let mut st = ctx.st.borrow_mut();
                // Release the previous app so WirePlumber can place it normally.
                if let Some(old) = st.bus_listener.remove(&bus) {
                    let nodes = resolve_capture(&st.graph, &old);
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
            // The listener changing invalidates any active MONITOR ON links
            // for this bus (step 3b's source is the bus's listener's own
            // playback, not the bus's own node) — tear down links from the
            // OLD listener's playback explicitly, since `ensure_links` only
            // adds missing pairs and wouldn't otherwise notice the desired
            // source itself changed out from under an unchanged Slot key.
            if let Some(old) = old_key {
                let affected: Vec<usize> =
                    ctx.st.borrow().monitors.iter().filter(|(b, _)| *b == bus).map(|(_, a)| *a).collect();
                for a in affected {
                    let mut st = ctx.st.borrow_mut();
                    let dst = st.bus_nodes.get(&a).map(|(id, _)| *id);
                    if let Some(dst) = dst {
                        let srcs = resolve_source(&st.graph, &old);
                        for src in srcs {
                            remove_links_between(ctx, &mut st, src, dst);
                        }
                    }
                }
            }
            // `reconcile_all` already calls `apply_listeners` as its first
            // step — call it before `emit_listeners` (not after, redundantly
            // calling `apply_listeners` twice) so the reported listener list
            // reflects the just-drawn links.
            reconcile_all(ctx);
            emit_listeners(ctx);
        }
        PwCmd::SetStripListener { idx, app_key } => {
            {
                let mut st = ctx.st.borrow_mut();
                // Release the previous app so WirePlumber can place it normally.
                if let Some(old) = st.strip_listener.remove(&idx) {
                    let nodes = resolve_capture(&st.graph, &old);
                    for n in nodes {
                        if st.capture_targets.remove(&n).is_some() {
                            if let Some(md) = st.metadata.as_ref() {
                                md.set_property(n, "target.object", None, None);
                            }
                        }
                    }
                }
                if let Some(k) = app_key {
                    st.strip_listener.insert(idx, k);
                }
            }
            reconcile_all(ctx);
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

    /// Regression for the "no mix-minus" bug: a capture stream whose binary
    /// has drifted slightly from the key stored in `bus_listener` (e.g.
    /// "discord-canary" vs the assigned "discord") must still resolve, the
    /// same substring fallback `resolve_source` already had.
    #[test]
    fn resolve_capture_falls_back_to_substring_match() {
        let mut g = Graph::default();
        g.nodes.insert(10, app(10, "discord-canary", "Stream/Input/Audio"));
        let found = resolve_capture(&g, "discord");
        assert_eq!(found, vec![10], "substring match on capture key must still resolve the node");
    }

    #[test]
    fn resolve_capture_exact_match_still_works() {
        let mut g = Graph::default();
        g.nodes.insert(10, app(10, "discord", "Stream/Input/Audio"));
        assert_eq!(resolve_capture(&g, "discord"), vec![10]);
    }

    #[test]
    fn resolve_capture_no_match_returns_empty() {
        let mut g = Graph::default();
        g.nodes.insert(10, app(10, "discord", "Stream/Input/Audio"));
        assert!(resolve_capture(&g, "spotify").is_empty());
    }

    /// A playback (not capture) node with a matching key must never be
    /// returned by resolve_capture — capture and playback are disjoint sets.
    #[test]
    fn resolve_capture_ignores_playback_nodes() {
        let mut g = Graph::default();
        g.nodes.insert(20, app(20, "discord", "Stream/Output/Audio"));
        assert!(resolve_capture(&g, "discord").is_empty());
    }

    /// Regression for the "Spotify plays but the strip's fader does nothing"
    /// bug: an app fanned out to both its FerroMix strip and a stray
    /// destination (e.g. a pipewire-pulse role loopback) must report the
    /// stray one so it can be cut.
    #[test]
    fn stray_destinations_finds_non_strip_link() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 100, 200)); // app(100) -> strip(200), keep
        g.links.insert(2, link(2, 100, 900)); // app(100) -> loopback(900), stray
        assert_eq!(stray_destinations(&g, 100, &[200]), vec![900]);
    }

    #[test]
    fn stray_destinations_dedupes_stereo_links() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 100, 900)); // FL -> loopback
        g.links.insert(2, link(2, 100, 900)); // FR -> loopback (same dest)
        assert_eq!(stray_destinations(&g, 100, &[200]), vec![900]);
    }

    #[test]
    fn stray_destinations_empty_when_exclusively_on_strip() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 100, 200));
        g.links.insert(2, link(2, 100, 200));
        assert!(stray_destinations(&g, 100, &[200]).is_empty());
    }

    #[test]
    fn stray_destinations_ignores_unrelated_nodes_links() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 100, 200)); // our app -> our strip
        g.links.insert(2, link(2, 300, 400)); // unrelated app -> unrelated node
        assert!(stray_destinations(&g, 100, &[200]).is_empty());
    }

    /// Regression for a self-inflicted fight found live: with an app
    /// legitimately assigned to TWO strips at once, a single-`keep` version
    /// of this treated the second strip as "stray" relative to the first —
    /// every destination in `keep` must be preserved, not just one.
    #[test]
    fn stray_destinations_keeps_multiple_legitimate_targets() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 100, 200)); // app -> strip A, both legit
        g.links.insert(2, link(2, 100, 250)); // app -> strip B, both legit
        g.links.insert(3, link(3, 100, 900)); // app -> loopback, stray
        assert_eq!(stray_destinations(&g, 100, &[200, 250]), vec![900]);
    }

    /// Regression for the "B-bus routing doesn't work" bug: an app's mic
    /// capture node fed by both the real default microphone AND our B-bus
    /// must report the real mic as stray so it can be cut — otherwise the
    /// receiving app hears both mixed together forever.
    #[test]
    fn stray_sources_finds_real_mic_alongside_bus() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 500, 999)); // bus(500) -> app capture(999), keep
        g.links.insert(2, link(2, 600, 999)); // real mic(600) -> app capture(999), stray
        assert_eq!(stray_sources(&g, 999, &[500]), vec![600]);
    }

    #[test]
    fn stray_sources_dedupes_stereo_links() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 600, 999)); // FL
        g.links.insert(2, link(2, 600, 999)); // FR, same stray source
        assert_eq!(stray_sources(&g, 999, &[500]), vec![600]);
    }

    #[test]
    fn stray_sources_empty_when_exclusively_from_bus() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 500, 999));
        g.links.insert(2, link(2, 500, 999));
        assert!(stray_sources(&g, 999, &[500]).is_empty());
    }

    #[test]
    fn stray_sources_ignores_unrelated_nodes_links() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 500, 999)); // our bus -> our app capture
        g.links.insert(2, link(2, 300, 400)); // unrelated
        assert!(stray_sources(&g, 999, &[500]).is_empty());
    }

    /// Regression for the exact live bug found: Discord's mic was assigned as
    /// listener to BOTH B1(500) and B2(700) simultaneously. A single-`keep`
    /// version processed B1 first (cutting B2's link as "stray"), then
    /// processed B2 (cutting B1's link as "stray" since it was just removed
    /// and re-added), oscillating every reconcile pass. Both must be kept.
    #[test]
    fn stray_sources_keeps_multiple_legitimate_buses() {
        let mut g = Graph::default();
        g.links.insert(1, link(1, 500, 999)); // B1 -> Discord mic, legit
        g.links.insert(2, link(2, 700, 999)); // B2 -> Discord mic, legit
        g.links.insert(3, link(3, 600, 999)); // real mic -> Discord mic, stray
        assert_eq!(stray_sources(&g, 999, &[500, 700]), vec![600]);
    }
}
