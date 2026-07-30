//! Per-strip DSP: a PipeWire `filter-chain` node running a noise gate and
//! compressor. Each strip can own one of these, inserted between its source
//! and the strip sink, so gate/comp apply to everything on that strip.
//!
//! We build the filter-chain by loading `libpipewire-module-filter-chain` with
//! a `filter.graph` of three nodes — two mono gates (one per channel) feeding
//! a stereo compressor:
//!   builtin/noisegate → downward noise gate (Open/Close Threshold, Attack…).
//!     PipeWire's own builtin filter-graph plugin — verified against a live
//!     daemon; earlier code used the label "gate", which doesn't exist (the
//!     module fails to load with "cannot create label gate" — confirmed via
//!     PIPEWIRE_DEBUG=3). The real label is "noisegate" (see
//!     `libpipewire-module-filter-chain(7)`'s BUILTIN FILTERS section). It's
//!     MONO (one "In"/"Out" pair) — confirmed via the SPA control descriptor
//!     dump, so a stereo strip needs two instances, `gate_l`/`gate_r`. Its
//!     "Open Threshold"/"Close Threshold" ports are LINEAR AMPLITUDE (SPA
//!     range 0.0..1.0 — confirmed via `PIPEWIRE_DEBUG=3`'s control dump,
//!     `0.040000/0.000000/1.000000`), not dB — earlier code passed raw dB
//!     values (e.g. -60.0), which are below the port's minimum and get
//!     silently clamped to 0.0, so the gate loaded but every threshold was
//!     effectively the same regardless of the knob. Convert with `db_to_lin`.
//!   ladspa/sc4 → soft-knee stereo compressor (Threshold level (dB), Ratio
//!     (1:n), Attack/Release time (ms), Makeup gain (dB)) — confirmed via
//!     `analyseplugin sc4_1882.so`. PipeWire's builtin plugin set has NO
//!     compressor at all, so this uses the well-known SC4 LADSPA plugin from
//!     the `ladspa-swh-plugins` Fedora package — the same one used in
//!     published PipeWire filter-chain compressor recipes. It's genuinely
//!     stereo (`"Left/Right input"`/`"Left/Right output"`), unlike the gate.
//!     `ladspa-swh-plugins` must be installed at runtime (see
//!     packaging/ferromix2.spec's Requires). LADSPA `plugin` in the SPA-JSON
//!     is the `.so` basename without extension (`sc4_1882`, not
//!     `sc4_1882.so` or a path) — PipeWire appends `.so` and searches the
//!     LADSPA plugin path itself.
//!
//! The module presents an `Audio/Sink` (capture side, we write into) and an
//! `Audio/Source` (playback side, we read out), passive so WirePlumber leaves
//! the auto-routing to us. Control values are baked into the module's SPA-JSON
//! args at load time for the FIRST load of a strip's module; every knob move
//! or GATE/COMP toggle after that goes through `push_live_params` instead,
//! which reaches into the already-running filter-chain and updates control
//! values in place — no destroy/recreate, no dropout. This works because the
//! chain's topology never changes between on/off states (see
//! `filter_chain_args`'s doc comment: "off" is neutral control values, not a
//! different graph), so a value-only update is always valid against whatever
//! generation of the module is currently loaded. Confirmed against pipewire
//! 1.2.7's actual module-filter-chain.c source (not just its man page): the
//! module's capture-side stream applies an incoming `SPA_PARAM_Props` whose
//! `params` property is a flat "name", value, "name", value struct, resolved
//! through the exact same `find_port`-style "<node>:<control>" syntax used to
//! build the SPA-JSON below — meaning the `gate_l`/`gate_r`/`comp` node names
//! chosen there double as the live-update addressing scheme, no separate
//! discovery step needed.
//!
//! Future direction (not built yet): the fixed gate→compressor chain here is
//! a special case of a more general capability — `filter.graph`'s `nodes`
//! list can hold ANY `ladspa`/(newer PipeWire) `lv2` plugin, not just
//! `sc4`, the same mechanism a live pitch-shifter or other real-time effect
//! insert would use. Getting there needs `StripDsp` to become a plugin
//! chain (a `Vec` of {plugin, params}) instead of two hardcoded stages, plus
//! a way to enumerate installed LV2/LADSPA plugins and their real port
//! names/ranges for the GUI (this file's whole hard-won port-name/threshold-
//! units history above is exactly the class of bug that needs solving generically
//! instead of once per plugin). Deliberately not designed further than this
//! paragraph until there's a second concrete plugin to design against, not
//! just the gate/compressor.
use std::ffi::CString;
use std::ptr::NonNull;

use mixer_core::model::StripDsp;
use pipewire as pw;

/// An owned handle to one loaded `filter-chain` module instance. Dropping it
/// destroys the module and everything it created (the dsp.in/dsp.out nodes),
/// exactly like `Recorder`/`Tap` own their PipeWire-side lifetime elsewhere in
/// this crate. Only ever touched from the single PipeWire main-loop thread
/// that owns `WorkerState` — same threading assumption as every other raw
/// handle this crate keeps.
pub struct DspModule {
    ptr: NonNull<pw::sys::pw_impl_module>,
}

impl Drop for DspModule {
    fn drop(&mut self) {
        unsafe { pw::sys::pw_impl_module_destroy(self.ptr.as_ptr()) }
    }
}

/// Load a `libpipewire-module-filter-chain` instance for strip `idx`, wired
/// per `filter_chain_args`. The `pipewire` crate has no safe binding for
/// `pw_context_load_module` (it only wraps *binding* to a module glimpsed via
/// the registry, not *loading* one from our own process), so this is a small,
/// contained unsafe shim around the C API — the same one WirePlumber and
/// pipewire.conf's own `context.modules` section use under the hood.
pub fn load_filter_chain(
    context: &pw::context::ContextRc,
    idx: usize,
    dsp: &StripDsp,
) -> Result<DspModule, String> {
    let name = CString::new("libpipewire-module-filter-chain").unwrap();
    let args = CString::new(filter_chain_args(idx, dsp)).map_err(|e| e.to_string())?;
    let raw = unsafe {
        pw::sys::pw_context_load_module(
            context.as_raw_ptr(),
            name.as_ptr(),
            args.as_ptr(),
            std::ptr::null_mut(),
        )
    };
    NonNull::new(raw)
        .map(|ptr| DspModule { ptr })
        .ok_or_else(|| format!("pw_context_load_module(filter-chain) failed for strip {idx}"))
}

/// The name of the filter-chain sink we write into for strip `idx`.
pub fn dsp_input_name(idx: usize) -> String {
    format!("ferromix.dsp.{idx}.in")
}
/// The source we read the processed signal from for strip `idx`.
pub fn dsp_output_name(idx: usize) -> String {
    format!("ferromix.dsp.{idx}.out")
}

/// Convert a dB value to the linear amplitude the noisegate's threshold ports
/// actually expect (SPA range 0.0..1.0 — see the module docstring).
fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Gate open/close thresholds in the noisegate's native units (linear
/// amplitude, see `db_to_lin`) for the current knob position/on-off state.
/// Shared by the initial `filter_chain_args` SPA-JSON and `live_props_pod`'s
/// live-update pod so the two paths can never silently diverge on what a
/// given `StripDsp` actually means to the running plugin.
fn gate_thresholds_lin(dsp: &StripDsp) -> (f32, f32) {
    let (open_db, close_db) = if dsp.gate_on {
        let t = dsp.gate_threshold_db();
        (t, t - 6.0) // hysteresis: close 6 dB below open
    } else {
        (-90.0, -95.0) // effectively always open
    };
    (db_to_lin(open_db), db_to_lin(close_db))
}

/// Compressor threshold (dB) / ratio for the current knob position/on-off
/// state — see `gate_thresholds_lin`'s doc comment, same sharing rationale.
fn comp_params(dsp: &StripDsp) -> (f32, f32) {
    if dsp.comp_on {
        (dsp.comp_threshold_db(), dsp.comp_ratio())
    } else {
        (0.0, 1.0) // 1:1 = no compression
    }
}

/// Build the `args` string for `libpipewire-module-filter-chain` for one strip.
///
/// This is the SPA-JSON the module parses. Returns a string that can be passed
/// to `context.load_module`. The gate and compressor are always present; when a
/// stage is "off" we set neutral control values (gate threshold near-zero so
/// it never closes, compressor ratio 1:1) so the chain topology never changes
/// at runtime — only control values do, which is cheap and glitch-free.
pub fn filter_chain_args(idx: usize, dsp: &StripDsp) -> String {
    let (gate_open, gate_close) = gate_thresholds_lin(dsp);
    let (comp_thresh, comp_ratio) = comp_params(dsp);

    let input = dsp_input_name(idx);
    let output = dsp_output_name(idx);

    // SPA-JSON. Kept compact; the control keys match the real plugin
    // descriptors (verified live — see the module docstring), not guesses.
    format!(
        r#"{{
  node.description = "FerroMix DSP {idx}"
  media.name = "FerroMix DSP {idx}"
  filter.graph = {{
    nodes = [
      {{
        type = builtin
        name = gate_l
        label = noisegate
        control = {{
          "Open Threshold" = {gate_open}
          "Close Threshold" = {gate_close}
          "Attack (s)" = 0.005
          "Release (s)" = 0.15
          "Hold (s)" = 0.05
        }}
      }}
      {{
        type = builtin
        name = gate_r
        label = noisegate
        control = {{
          "Open Threshold" = {gate_open}
          "Close Threshold" = {gate_close}
          "Attack (s)" = 0.005
          "Release (s)" = 0.15
          "Hold (s)" = 0.05
        }}
      }}
      {{
        type = ladspa
        name = comp
        plugin = sc4_1882
        label = sc4
        control = {{
          "Threshold level (dB)" = {comp_thresh}
          "Ratio (1:n)" = {comp_ratio}
          "Attack time (ms)" = 10
          "Release time (ms)" = 200
          "Makeup gain (dB)" = 0
        }}
      }}
    ]
    links = [
      {{ output = "gate_l:Out" input = "comp:Left input" }}
      {{ output = "gate_r:Out" input = "comp:Right input" }}
    ]
    inputs = [ "gate_l:In" "gate_r:In" ]
    outputs = [ "comp:Left output" "comp:Right output" ]
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

/// Build the `SPA_PARAM_Props` pod bytes for a live control update — pure
/// and independently testable from the actual `node.set_param` call in
/// `push_live_params` below. The `params` property's value is a flat Struct
/// of alternating "<node>:<control>" name strings and float values (NOT a
/// SPA array — a heterogeneous struct, matching module-filter-chain.c's
/// `parse_params()`, which walks it with alternating
/// `spa_pod_parser_get_string`/`get_float` calls). Every control this DSP
/// chain has is included on every call, on or off — cheaper to always send
/// the full set than to track which control actually changed, and this
/// runs at most once per user gesture (see widgets.rs's `DialState`), not
/// per audio buffer.
fn live_props_pod(dsp: &StripDsp) -> Result<Vec<u8>, String> {
    use pw::spa::pod::{serialize::PodSerializer, Object, Property, PropertyFlags, Value};
    let (gate_open, gate_close) = gate_thresholds_lin(dsp);
    let (comp_thresh, comp_ratio) = comp_params(dsp);
    let params = Value::Struct(vec![
        Value::String("gate_l:Open Threshold".into()),
        Value::Float(gate_open),
        Value::String("gate_l:Close Threshold".into()),
        Value::Float(gate_close),
        Value::String("gate_r:Open Threshold".into()),
        Value::Float(gate_open),
        Value::String("gate_r:Close Threshold".into()),
        Value::Float(gate_close),
        Value::String("comp:Threshold level (dB)".into()),
        Value::Float(comp_thresh),
        Value::String("comp:Ratio (1:n)".into()),
        Value::Float(comp_ratio),
    ]);
    let obj = Object {
        type_: pw::spa::sys::SPA_TYPE_OBJECT_Props,
        id: pw::spa::sys::SPA_PARAM_Props,
        properties: vec![Property {
            key: pw::spa::sys::SPA_PROP_params,
            flags: PropertyFlags::empty(),
            value: params,
        }],
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| format!("pod serialize: {e:?}"))
}

/// Push new gate/compressor control values into an ALREADY-LOADED strip's
/// filter-chain module without destroying/reloading it — see this module's
/// doc comment for why that's safe (topology never changes) and how PipeWire
/// resolves the addressing. `node` must be the bound proxy for that strip's
/// `dsp_input_name` node (the module's capture-side `Audio/Sink`, which is
/// where module-filter-chain.c's live-Props handling is wired) — the caller
/// (`worker.rs`) is responsible for only calling this once that node has
/// actually shown up in the registry and been bound.
pub fn push_live_params(node: &pw::node::Node, dsp: &StripDsp) -> Result<(), String> {
    let bytes = live_props_pod(dsp)?;
    let pod = pw::spa::pod::Pod::from_bytes(&bytes).ok_or("pod from_bytes failed")?;
    node.set_param(pw::spa::param::ParamType::Props, 0, pod);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_lin_known_points() {
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_lin(-20.0) - 0.1).abs() < 1e-6);
        assert!((db_to_lin(-60.0) - 0.001).abs() < 1e-9);
    }

    #[test]
    fn gate_off_is_always_open() {
        let dsp = StripDsp { gate_on: false, gate: 0.5, comp_on: false, comp: 0.5 };
        let args = filter_chain_args(0, &dsp);
        // gate off → near-zero linear threshold so it never closes
        let open = format!("Open Threshold\" = {}", db_to_lin(-90.0));
        assert!(args.contains(&open), "expected {open:?} in:\n{args}");
        // comp off → ratio 1:1
        assert!(args.contains("Ratio (1:n)\" = 1"));
    }

    #[test]
    fn gate_on_uses_knob_threshold() {
        let dsp = StripDsp { gate_on: true, gate: 0.0, comp_on: false, comp: 0.0 };
        // gate knob 0.0 → -60 dB open threshold → linear 0.001 (SPA range is
        // linear amplitude, not dB — see the module docstring).
        let args = filter_chain_args(0, &dsp);
        assert!(args.contains("Open Threshold\" = 0.001"));
        // close threshold is 6 dB below → linear ~0.000501
        let close = format!("Close Threshold\" = {}", db_to_lin(-66.0));
        assert!(args.contains(&close), "expected {close:?} in:\n{args}");
    }

    #[test]
    fn comp_on_maps_knob_to_ratio_and_threshold() {
        let dsp = StripDsp { gate_on: false, gate: 0.0, comp_on: true, comp: 1.0 };
        let args = filter_chain_args(2, &dsp);
        // comp knob 1.0 → ratio 8:1, threshold -30 dB (SC4's real port names).
        assert!(args.contains("Ratio (1:n)\" = 8"));
        assert!(args.contains("Threshold level (dB)\" = -30"));
        // node names carry the strip index
        assert!(args.contains("ferromix.dsp.2.in"));
        assert!(args.contains("ferromix.dsp.2.out"));
    }

    #[test]
    fn live_props_pod_round_trips_to_the_same_values_as_initial_load() {
        // The live-update path and the initial SPA-JSON load path must never
        // disagree on what a given StripDsp means — they're read by the SAME
        // C-side control resolution (module-filter-chain.c's find_port
        // "node:control" syntax), just delivered two different ways. Parse
        // the live pod back and check its (name, value) pairs against the
        // same dsp.rs helpers `filter_chain_args` uses.
        use pw::spa::pod::deserialize::PodDeserializer;
        use pw::spa::pod::Value;

        for dsp in [
            StripDsp { gate_on: true, gate: 0.3, comp_on: true, comp: 0.7 },
            StripDsp { gate_on: false, gate: 0.9, comp_on: false, comp: 0.1 },
        ] {
            let bytes = live_props_pod(&dsp).expect("serialize");
            let (_, value) = PodDeserializer::deserialize_from::<Value>(&bytes).expect("deserialize");
            let Value::Object(obj) = value else { panic!("expected an Object pod") };
            assert_eq!(obj.type_, pw::spa::sys::SPA_TYPE_OBJECT_Props);
            assert_eq!(obj.id, pw::spa::sys::SPA_PARAM_Props);
            let params_prop = obj
                .properties
                .iter()
                .find(|p| p.key == pw::spa::sys::SPA_PROP_params)
                .expect("params property present");
            let Value::Struct(fields) = &params_prop.value else { panic!("params value should be a Struct") };

            let get = |name: &str| -> f32 {
                let i = fields
                    .iter()
                    .position(|v| matches!(v, Value::String(s) if s == name))
                    .unwrap_or_else(|| panic!("{name} not found in {fields:?}"));
                match &fields[i + 1] {
                    Value::Float(f) => *f,
                    other => panic!("expected float after {name}, got {other:?}"),
                }
            };

            let (gate_open, gate_close) = gate_thresholds_lin(&dsp);
            let (comp_thresh, comp_ratio) = comp_params(&dsp);
            assert_eq!(get("gate_l:Open Threshold"), gate_open);
            assert_eq!(get("gate_l:Close Threshold"), gate_close);
            assert_eq!(get("gate_r:Open Threshold"), gate_open);
            assert_eq!(get("gate_r:Close Threshold"), gate_close);
            assert_eq!(get("comp:Threshold level (dB)"), comp_thresh);
            assert_eq!(get("comp:Ratio (1:n)"), comp_ratio);

            // Cross-check against the initial-load SPA-JSON's own numbers too
            // (belt and suspenders — this is the exact regression the shared
            // helpers exist to prevent).
            let args = filter_chain_args(3, &dsp);
            assert!(args.contains(&format!("\"Open Threshold\" = {gate_open}")));
            assert!(args.contains(&format!("\"Threshold level (dB)\" = {comp_thresh}")));
        }
    }

    #[test]
    fn topology_wires_stereo_gates_into_stereo_compressor() {
        // The noisegate builtin is mono; the SC4 compressor is stereo. A
        // stereo strip needs two gate instances feeding the compressor's
        // Left/Right inputs — a single "gate:Out" -> "comp:In" link (the
        // original, wrong topology) doesn't match either plugin's real ports.
        let dsp = StripDsp::default();
        let args = filter_chain_args(0, &dsp);
        assert!(args.contains("name = gate_l"));
        assert!(args.contains("name = gate_r"));
        assert!(args.contains("plugin = sc4_1882"));
        assert!(args.contains("label = sc4"));
        assert!(args.contains(r#"{ output = "gate_l:Out" input = "comp:Left input" }"#));
        assert!(args.contains(r#"{ output = "gate_r:Out" input = "comp:Right input" }"#));
        assert!(args.contains(r#"inputs = [ "gate_l:In" "gate_r:In" ]"#));
        assert!(args.contains(r#"outputs = [ "comp:Left output" "comp:Right output" ]"#));
    }
}
