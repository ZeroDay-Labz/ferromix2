//! Virtual device creation and node property control.
//!
//! Strips and A-buses are `support.null-audio-sink` adapters with
//! `media.class = Audio/Sink`; B-buses use `Audio/Source/Virtual` so apps see
//! them as a microphone.
//!
//! Deliberately NOT lingered: a lingering node outlives the daemon, so every
//! restart used to leave another copy behind ("FerroMix A1", "FerroMix A1-1",
//! ...) and the reconciler would wire links to one copy while the taps watched
//! another. Our devices now die with the daemon and are rebuilt from config on
//! the next start, which is both simpler and self-healing.

use pipewire as pw;
use pw::properties::properties;
use std::io::Cursor;

#[allow(dead_code)]
pub fn strip_node_name(idx: usize) -> String {
    format!("ferromix.strip.{idx}")
}
pub fn bus_node_name(idx: usize) -> String {
    format!("ferromix.bus.{idx}")
}

/// Create a virtual sink (strip / hardware bus). Returns the proxy, which we
/// keep alive in the worker state. `rate` should match whatever the graph's
/// clock is currently forced to (`Config.sample_rate` /
/// `Command::SetSampleRate`, default 48000) — see the `audio.rate` comment
/// below for why this matters.
pub fn create_sink(core: &pw::core::CoreRc, name: &str, desc: &str, rate: u32) -> Result<pw::node::Node, String> {
    let rate_s = rate.to_string();
    let props = properties! {
        "factory.name" => "support.null-audio-sink",
        "node.name" => name,
        "node.description" => desc,
        "media.class" => "Audio/Sink",
        "audio.position" => "[ FL FR ]",
        // monitor follows the sink's channelVolumes → our fader affects routing
        "monitor.channel-volumes" => "true",
        "node.virtual" => "true",
        // Pin an exact rate and a high-quality resampler instead of leaving
        // both at PipeWire's defaults (~4/14 quality). A typical signal path
        // chains 2-4 of these adapter nodes back to back (source -> strip ->
        // [DSP] -> bus -> hw); each one is an independent resample/format
        // point, and default-quality resampling compounded across that many
        // hops is a known source of audible smearing ("underwater"/phasey
        // sound) even when every hop individually sounds fine in isolation.
        // Mismatching this against the graph's actual forced rate would
        // reintroduce exactly that problem, so it must track the live
        // setting rather than staying a hardcoded 48000.
        "audio.rate" => rate_s.as_str(),
        "resample.quality" => "10",
    };
    core.create_object::<pw::node::Node>("adapter", &props)
        .map_err(|e| format!("create sink {name}: {e}"))
}

/// Create a virtual source (B bus / virtual mic apps can select).
///
/// This is a null-audio-sink presented as a source: apps read its capture
/// ports as a microphone, while FerroMix writes strip audio into its *sink*
/// (playback) side.
///
/// Critically, a virtual source's monitor is auto-linked to the default output
/// by WirePlumber's default policy — which is why you could *hear* B1/B2/B3 in
/// your headphones. A virtual mic must be SILENT to the user: only FerroMix's
/// explicit links (strip→bus sends, app captures, opt-in MONITOR) should ever
/// touch it. So we disable autoconnect and suppress the monitor entirely.
pub fn create_virtual_source(
    core: &pw::core::CoreRc,
    name: &str,
    desc: &str,
    rate: u32,
) -> Result<pw::node::Node, String> {
    let rate_s = rate.to_string();
    let props = properties! {
        "factory.name" => "support.null-audio-sink",
        "node.name" => name,
        "node.description" => desc,
        "media.class" => "Audio/Source/Virtual",
        "audio.position" => "[ FL FR ]",
        // Do NOT let the session manager wire this anywhere. A virtual mic is
        // driven only by FerroMix's own links; without this, its monitor gets
        // patched to your speakers and you hear yourself.
        "node.autoconnect" => "false",
        "node.dont-reconnect" => "true",
        // No monitor ports at all — nothing to leak into the default sink.
        "monitor.channel-volumes" => "false",
        "audio.adapt.follower.monitor" => "false",
        "monitor.passthrough" => "false",
        "channelmix.normalize" => "false",
        "node.virtual" => "true",
        // See create_sink's comment — same cumulative-resample-quality fix.
        "audio.rate" => rate_s.as_str(),
        "resample.quality" => "10",
    };
    core.create_object::<pw::node::Node>("adapter", &props)
        .map_err(|e| format!("create virtual source {name}: {e}"))
}

/// UI fader position → linear channel volume, using the same -60..+12 dB law
/// the GUI prints beside the fader. (A cubic taper used to be applied here,
/// which meant the number on screen was a lie and there was no headroom.)
pub fn taper(ui: f32) -> f32 {
    mixer_core::model::pos_to_gain(ui)
}

/// Set channelVolumes on a node via a SPA `Props` param pod.
pub fn set_node_volume(node: &pw::node::Node, ui_volume: f32) -> Result<(), String> {
    use pw::spa::pod::{serialize::PodSerializer, Object, Pod, Property, PropertyFlags, Value, ValueArray};
    let v = taper(ui_volume);
    let obj = Object {
        type_: pw::spa::sys::SPA_TYPE_OBJECT_Props,
        id: pw::spa::sys::SPA_PARAM_Props,
        properties: vec![Property {
            key: pw::spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(vec![v, v])),
        }],
    };
    let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
        .map_err(|e| format!("pod serialize: {e:?}"))?
        .0
        .into_inner();
    let pod = Pod::from_bytes(&bytes).ok_or("pod from_bytes failed")?;
    node.set_param(pw::spa::param::ParamType::Props, 0, pod);
    Ok(())
}

/// Set mute on a node via a SPA `Props` param pod.
pub fn set_node_mute(node: &pw::node::Node, mute: bool) -> Result<(), String> {
    use pw::spa::pod::{serialize::PodSerializer, Object, Pod, Property, PropertyFlags, Value};
    let obj = Object {
        type_: pw::spa::sys::SPA_TYPE_OBJECT_Props,
        id: pw::spa::sys::SPA_PARAM_Props,
        properties: vec![Property {
            key: pw::spa::sys::SPA_PROP_mute,
            flags: PropertyFlags::empty(),
            value: Value::Bool(mute),
        }],
    };
    let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
        .map_err(|e| format!("pod serialize: {e:?}"))?
        .0
        .into_inner();
    let pod = Pod::from_bytes(&bytes).ok_or("pod from_bytes failed")?;
    node.set_param(pw::spa::param::ParamType::Props, 0, pod);
    Ok(())
}
