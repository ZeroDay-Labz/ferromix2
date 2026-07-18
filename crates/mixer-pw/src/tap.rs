//! VU meter taps: a passive capture stream per bus (and the mic) that computes
//! a peak and ships throttled Level events. `node.passive` = never forces the
//! graph to run just to draw meters.

use mixer_core::backend::BackendEvent;
use mixer_core::model::{Level, LevelKey};
use pipewire as pw;
use pw::properties::properties;
use std::io::Cursor;
use std::sync::mpsc::Sender;

pub struct Tap {
    _stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<TapData>,
}

struct TapData {
    tx: Sender<BackendEvent>,
    key: LevelKey,
    /// Per-channel peak (we negotiate F32LE stereo, so samples interleave L,R).
    peak_l: f32,
    peak_r: f32,
    bufs: u32,
}

pub fn f32_format_pod() -> Vec<u8> {
    let mut info = pw::spa::param::audio::AudioInfoRaw::new();
    info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
    info.set_rate(48_000);
    info.set_channels(2);
    let obj = pw::spa::pod::Object {
        type_: pw::spa::sys::SPA_TYPE_OBJECT_Format,
        id: pw::spa::sys::SPA_PARAM_EnumFormat,
        properties: info.into(),
    };
    pw::spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &pw::spa::pod::Value::Object(obj))
        .expect("serialize format pod")
        .0
        .into_inner()
}

impl Tap {
    /// `capture_sink`: true to tap a sink's monitor (buses/A-out), false to tap
    /// a source's output (mic / virtual mic).
    pub fn new(
        core: &pw::core::CoreRc,
        key: LevelKey,
        target_node_name: &str,
        capture_sink: bool,
        tx: Sender<BackendEvent>,
    ) -> Result<Tap, String> {
        let name = match key {
            LevelKey::Strip(i) => format!("ferromix.tap.strip{i}"),
            LevelKey::Bus(i) => format!("ferromix.tap.bus{i}"),
        };
        let props = properties! {
            "media.type" => "Audio",
            "media.category" => "Capture",
            "media.role" => "DSP",
            "node.name" => name.as_str(),
            "target.object" => target_node_name,
            "stream.capture.sink" => if capture_sink { "true" } else { "false" },
            "node.passive" => "true",
        };
        let stream = pw::stream::StreamRc::new(core.clone(), &name, props).map_err(|e| format!("tap {name}: {e}"))?;

        let listener = stream
            .add_local_listener_with_user_data(TapData { tx, key, peak_l: 0.0, peak_r: 0.0, bufs: 0 })
            .process(|stream, ud| {
                if let Some(mut buffer) = stream.dequeue_buffer() {
                    for data in buffer.datas_mut() {
                        let valid = data.chunk().size() as usize;
                        if let Some(bytes) = data.data() {
                            let n = valid.min(bytes.len()) / 4;
                            let samples = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, n) };
                            for (i, s) in samples.iter().enumerate() {
                                let a = s.abs();
                                // interleaved stereo: even = L, odd = R
                                if i % 2 == 0 {
                                    if a > ud.peak_l {
                                        ud.peak_l = a;
                                    }
                                } else if a > ud.peak_r {
                                    ud.peak_r = a;
                                }
                            }
                        }
                    }
                    ud.bufs += 1;
                    if ud.bufs >= 2 {
                        if ud.peak_l > 0.0008 || ud.peak_r > 0.0008 {
                            let lv = Level { l: ud.peak_l.min(1.0), r: ud.peak_r.min(1.0) };
                            let _ = ud.tx.send(BackendEvent::Level(ud.key.clone(), lv));
                        }
                        ud.bufs = 0;
                        ud.peak_l = 0.0;
                        ud.peak_r = 0.0;
                    }
                }
            })
            .register()
            .map_err(|e| format!("tap listener {name}: {e}"))?;

        let pod_bytes = f32_format_pod();
        let mut params = [pw::spa::pod::Pod::from_bytes(&pod_bytes).ok_or("format pod parse failed")?];
        stream
            .connect(
                pw::spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
                &mut params,
            )
            .map_err(|e| format!("tap connect {name}: {e}"))?;

        Ok(Tap { _stream: stream, _listener: listener })
    }
}
