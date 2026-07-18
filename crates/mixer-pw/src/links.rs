//! Channel-aware port pairing + link creation. Link proxies are held by the
//! worker; dropping them tears the link down (we don't linger links, so
//! stopping the daemon = clean silence, rebuilt on next start).

use crate::registry::{Graph, NodeId};
use pipewire as pw;
use pw::properties::properties;

/// Pair (out_port, in_port) between two nodes.
///
/// Three cases, in order:
///   * **exact channel match** (FL→FL, FR→FR) when both sides are multichannel,
///   * **mono fan-out**: a single source port is duplicated into *every* input
///     channel, so a mono mic lands in BOTH L and R instead of hard-panning to
///     one ear (this was the "audio only in my right ear" bug),
///   * **positional** fallback when channels aren't labelled.
pub fn pair_ports(graph: &Graph, out_node: NodeId, in_node: NodeId) -> Vec<(u32, u32)> {
    let outs = graph.out_ports(out_node);
    let ins = graph.in_ports(in_node);
    if outs.is_empty() || ins.is_empty() {
        return Vec::new();
    }

    // Mono source → fan its single port into every destination input channel.
    // Without this, `find(channel == channel)` matches only FL (or nothing),
    // and the signal collapses into one ear.
    if outs.len() == 1 {
        return ins.iter().map(|i| (outs[0].id, i.id)).collect();
    }

    // Mono destination → sum every source channel into the one input port.
    if ins.len() == 1 {
        return outs.iter().map(|o| (o.id, ins[0].id)).collect();
    }

    // Multichannel both ways: match by channel label, else by position.
    let mut pairs = Vec::new();
    for (idx, o) in outs.iter().enumerate() {
        let target = ins
            .iter()
            .find(|i| i.channel.is_some() && i.channel == o.channel)
            .or_else(|| ins.get(idx.min(ins.len() - 1)));
        if let Some(i) = target {
            pairs.push((o.id, i.id));
        }
    }
    pairs
}

pub fn create_link(
    core: &pw::core::CoreRc,
    out_node: NodeId,
    out_port: u32,
    in_node: NodeId,
    in_port: u32,
) -> Result<pw::link::Link, String> {
    let props = properties! {
        "link.output.node" => out_node.to_string(),
        "link.output.port" => out_port.to_string(),
        "link.input.node" => in_node.to_string(),
        "link.input.port" => in_port.to_string(),
    };
    core.create_object::<pw::link::Link>("link-factory", &props)
        .map_err(|e| format!("link {out_port}->{in_port}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Dir, Graph, PwPort};

    fn port(id: u32, node: u32, dir: Dir, ch: Option<&str>) -> PwPort {
        PwPort { id, node, dir, channel: ch.map(str::to_string), is_monitor: false }
    }

    fn graph_with(ports: Vec<PwPort>) -> Graph {
        let mut g = Graph::default();
        for p in ports {
            g.ports.insert(p.id, p);
        }
        g
    }

    #[test]
    fn mono_source_fans_into_both_stereo_inputs() {
        // mono mic (node 1, one MONO out) -> stereo bus (node 2, FL+FR in)
        let g = graph_with(vec![
            port(10, 1, Dir::Out, Some("MONO")),
            port(20, 2, Dir::In, Some("FL")),
            port(21, 2, Dir::In, Some("FR")),
        ]);
        let pairs = pair_ports(&g, 1, 2);
        assert_eq!(pairs.len(), 2, "mono must reach BOTH channels, not one ear");
        assert!(pairs.contains(&(10, 20)));
        assert!(pairs.contains(&(10, 21)));
    }

    #[test]
    fn stereo_matches_by_channel_label() {
        let g = graph_with(vec![
            port(10, 1, Dir::Out, Some("FL")),
            port(11, 1, Dir::Out, Some("FR")),
            port(20, 2, Dir::In, Some("FL")),
            port(21, 2, Dir::In, Some("FR")),
        ]);
        let mut pairs = pair_ports(&g, 1, 2);
        pairs.sort();
        assert_eq!(pairs, vec![(10, 20), (11, 21)]);
    }

    #[test]
    fn stereo_source_sums_into_mono_destination() {
        let g = graph_with(vec![
            port(10, 1, Dir::Out, Some("FL")),
            port(11, 1, Dir::Out, Some("FR")),
            port(20, 2, Dir::In, Some("MONO")),
        ]);
        let pairs = pair_ports(&g, 1, 2);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(10, 20)));
        assert!(pairs.contains(&(11, 20)));
    }
}
