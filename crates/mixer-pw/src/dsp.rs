//! Per-strip DSP: a PipeWire `filter-chain` node running the builtin noise gate
//! and compressor. Each strip can own one of these, inserted between its source
//! and the strip sink, so gate/comp apply to everything on that strip.
//!
//! We build the filter-chain by loading `libpipewire-module-filter-chain` with a
//! `filter.graph` of two builtin nodes:
//!   builtin/gate       → downward noise gate (Open/Close threshold, Attack…)
//!   builtin/compressor → soft-knee compressor (Threshold, Ratio, …)
//!
//! The module presents an `Audio/Sink` (capture side, we write into) and an
//! `Audio/Source` (playback side, we read out), passive so WirePlumber leaves
//! the auto-routing to us. Control ports are updated live via the node's params
//! without rebuilding the chain.

use mixer_core::model::StripDsp;

/// The name of the filter-chain sink we write into for strip `idx`.
pub fn dsp_input_name(idx: usize) -> String {
    format!("ferromix.dsp.{idx}.in")
}
/// The source we read the processed signal from for strip `idx`.
pub fn dsp_output_name(idx: usize) -> String {
    format!("ferromix.dsp.{idx}.out")
}

/// Build the `args` string for `libpipewire-module-filter-chain` for one strip.
///
/// This is the SPA-JSON the module parses. Returns a string that can be passed
/// to `context.load_module`. The gate and compressor are always present; when a
/// stage is "off" we set neutral control values (gate threshold at -inf-ish,
/// compressor ratio 1:1) so the chain topology never changes at runtime — only
/// control values do, which is cheap and glitch-free.
pub fn filter_chain_args(idx: usize, dsp: &StripDsp) -> String {
    let (gate_open, gate_close) = if dsp.gate_on {
        let t = dsp.gate_threshold_db();
        (t, t - 6.0) // hysteresis: close 6 dB below open
    } else {
        (-90.0, -95.0) // effectively always open
    };

    let (comp_thresh, comp_ratio) = if dsp.comp_on {
        (dsp.comp_threshold_db(), dsp.comp_ratio())
    } else {
        (0.0, 1.0) // 1:1 = no compression
    };

    let input = dsp_input_name(idx);
    let output = dsp_output_name(idx);

    // SPA-JSON. Kept compact; the control keys match PipeWire's builtin filters.
    format!(
        r#"{{
  node.description = "FerroMix DSP {idx}"
  media.name = "FerroMix DSP {idx}"
  filter.graph = {{
    nodes = [
      {{
        type = builtin
        name = gate
        label = gate
        control = {{
          "Open threshold" = {gate_open}
          "Close threshold" = {gate_close}
          "Attack (s)" = 0.005
          "Release (s)" = 0.15
          "Hold (s)" = 0.05
        }}
      }}
      {{
        type = builtin
        name = comp
        label = compressor
        control = {{
          "Threshold (dB)" = {comp_thresh}
          "Ratio" = {comp_ratio}
          "Attack (s)" = 0.01
          "Release (s)" = 0.2
          "Makeup (dB)" = 0.0
        }}
      }}
    ]
    links = [
      {{ output = "gate:Out" input = "comp:In" }}
    ]
    inputs = [ "gate:In" ]
    outputs = [ "comp:Out" ]
  }}
  capture.props = {{
    node.name = "{input}"
    media.class = Audio/Sink
    node.passive = true
    audio.position = [ FL FR ]
  }}
  playback.props = {{
    node.name = "{output}"
    media.class = Audio/Source
    node.passive = true
    audio.position = [ FL FR ]
  }}
}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_off_is_always_open() {
        let dsp = StripDsp { gate_on: false, gate: 0.5, comp_on: false, comp: 0.5 };
        let args = filter_chain_args(0, &dsp);
        // gate off → very low open threshold so nothing is gated
        assert!(args.contains("Open threshold\" = -90"));
        // comp off → ratio 1:1
        assert!(args.contains("Ratio\" = 1"));
    }

    #[test]
    fn gate_on_uses_knob_threshold() {
        let dsp = StripDsp { gate_on: true, gate: 0.0, comp_on: false, comp: 0.0 };
        // gate knob 0.0 → -60 dB open threshold
        let args = filter_chain_args(0, &dsp);
        assert!(args.contains("Open threshold\" = -60"));
        // close threshold is 6 dB below
        assert!(args.contains("Close threshold\" = -66"));
    }

    #[test]
    fn comp_on_maps_knob_to_ratio_and_threshold() {
        let dsp = StripDsp { gate_on: false, gate: 0.0, comp_on: true, comp: 1.0 };
        let args = filter_chain_args(2, &dsp);
        // comp knob 1.0 → ratio 8:1, threshold -30 dB
        assert!(args.contains("Ratio\" = 8"));
        assert!(args.contains("Threshold (dB)\" = -30"));
        // node names carry the strip index
        assert!(args.contains("ferromix.dsp.2.in"));
        assert!(args.contains("ferromix.dsp.2.out"));
    }
}
