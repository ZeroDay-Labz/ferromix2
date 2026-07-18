//! Bus recording → 32-bit float WAV via hound. A non-passive capture stream on
//! the bus's monitor keeps the graph running while recording.

use pipewire as pw;
use pw::properties::properties;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::{Arc, Mutex};

type Writer = hound::WavWriter<BufWriter<File>>;

pub struct Recorder {
    _stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<RecData>,
    writer: Arc<Mutex<Option<Writer>>>,
}

struct RecData {
    writer: Arc<Mutex<Option<Writer>>>,
}

impl Recorder {
    pub fn new(core: &pw::core::CoreRc, target_node_name: &str, capture_sink: bool, path: &Path) -> Result<Recorder, String> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let writer = hound::WavWriter::create(path, spec).map_err(|e| format!("wav create: {e}"))?;
        let writer = Arc::new(Mutex::new(Some(writer)));

        let name = format!("ferromix.rec.{}", target_node_name.replace('.', "-"));
        let props = properties! {
            "media.type" => "Audio",
            "media.category" => "Capture",
            "media.role" => "Production",
            "node.name" => name.as_str(),
            "target.object" => target_node_name,
            "stream.capture.sink" => if capture_sink { "true" } else { "false" },
        };
        let stream = pw::stream::StreamRc::new(core.clone(), &name, props).map_err(|e| format!("rec stream: {e}"))?;

        let listener = stream
            .add_local_listener_with_user_data(RecData { writer: Arc::clone(&writer) })
            .process(|stream, ud| {
                if let Some(mut buffer) = stream.dequeue_buffer() {
                    let mut guard = match ud.writer.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    let Some(w) = guard.as_mut() else { return };
                    for data in buffer.datas_mut() {
                        let valid = data.chunk().size() as usize;
                        if let Some(bytes) = data.data() {
                            let n = valid.min(bytes.len()) / 4;
                            let samples = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, n) };
                            for s in samples {
                                let _ = w.write_sample(*s);
                            }
                        }
                    }
                }
            })
            .register()
            .map_err(|e| format!("rec listener: {e}"))?;

        let pod_bytes = crate::tap::f32_format_pod();
        let mut params = [pw::spa::pod::Pod::from_bytes(&pod_bytes).ok_or("format pod parse failed")?];
        stream
            .connect(
                pw::spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
                &mut params,
            )
            .map_err(|e| format!("rec connect: {e}"))?;

        Ok(Recorder { _stream: stream, _listener: listener, writer })
    }

    pub fn stop(&mut self) -> Result<(), String> {
        let taken = self.writer.lock().map_err(|_| "writer poisoned")?.take();
        if let Some(w) = taken {
            w.finalize().map_err(|e| format!("wav finalize: {e}"))?;
        }
        Ok(())
    }
}
