use mixer_core::engine::Command;
use mixer_core::ipc::{self, Request};

fn main() {
    let mut stream = std::os::unix::net::UnixStream::connect(
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into()) + "/ferromix2.sock",
    ).expect("connect");
    ipc::write_frame(&mut stream, &Request::Cmd(Command::SetSampleRate { rate: 48_000 })).unwrap();
}
