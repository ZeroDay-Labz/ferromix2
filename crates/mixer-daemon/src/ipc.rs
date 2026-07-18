//! Unix socket server: length-prefixed bincode frames, request/response.
//! One thread per client; the protocol lives in `mixer_core::ipc`.

use mixer_core::engine::EngineHandle;
use mixer_core::ipc::{read_frame, socket_path, write_frame, Request, Response};
use std::os::unix::net::{UnixListener, UnixStream};

pub fn serve(handle: EngineHandle) -> ! {
    let path = socket_path();
    let _ = std::fs::remove_file(&path); // stale socket from a previous run

    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            log::error!("cannot bind {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    log::info!("ipc listening on {}", path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let h = handle.clone();
                std::thread::Builder::new()
                    .name("ferromix-ipc-client".into())
                    .spawn(move || client(s, h))
                    .ok();
            }
            Err(e) => log::warn!("accept: {e}"),
        }
    }
    unreachable!("listener.incoming() never ends");
}

fn client(mut stream: UnixStream, handle: EngineHandle) {
    loop {
        let req: Request = match read_frame(&mut stream) {
            Ok(r) => r,
            Err(_) => return, // client hung up
        };
        let resp = match req {
            Request::GetState => Response::State(handle.snapshot()),
            Request::Cmd(cmd) => {
                handle.send(cmd);
                Response::Ok
            }
            Request::Ping => Response::Pong,
        };
        if write_frame(&mut stream, &resp).is_err() {
            return;
        }
    }
}
