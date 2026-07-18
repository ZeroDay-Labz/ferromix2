//! ferromix-gui entrypoint.
//!
//! * Linux: connects to the daemon's Unix socket (LIVE). If the daemon isn't
//!   running, falls back to the in-process mock so the UI still opens.
//! * Windows/macOS or `--mock`: runs the engine in-process against the
//!   MockBackend — full GUI development with fake apps and animated meters.

mod app;
mod controller;
mod theme;
mod ui;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Make any panic loud and unmissable on stderr, with a backtrace, instead
    // of the process vanishing with no window and no message.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("\n╔══════════════════════════════════════════════╗");
        eprintln!("║  FERROMIX GUI PANIC — this is why no window   ║");
        eprintln!("╚══════════════════════════════════════════════╝");
        eprintln!("{info}");
        eprintln!("backtrace:\n{}", std::backtrace::Backtrace::force_capture());
    }));

    log::info!("step 1/4: parsing args");
    let force_mock = std::env::args().any(|a| a == "--mock");

    log::info!("step 2/4: building controller (force_mock={force_mock})");
    let controller = controller::make(force_mock);

    log::info!("step 3/4: loading config");
    let saved_scale = mixer_core::Config::load_or_create().ui_scale;

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1620.0, 760.0])
            .with_min_inner_size([1100.0, 560.0])
            .with_resizable(true)
            .with_app_id("ferromix")
            .with_title("FERROMIX"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    log::info!("step 4/4: calling run_native (window should appear now)");

    // The app builder is needed for a possible retry, so make it a fn of the
    // controller. run_native consumes the closure, so we build the controller
    // into an Option we can take twice if needed.
    let controller = std::cell::RefCell::new(Some(controller));
    let make_builder = || {
        let taken = controller.borrow_mut().take().expect("controller taken twice");
        move |cc: &eframe::CreationContext<'_>| {
            let native_ppp = cc.egui_ctx.pixels_per_point();
            let scale = if saved_scale > 0.05 { saved_scale } else { native_ppp };
            log::info!("GUI creation ctx OK: native_ppp={native_ppp:.2}, scale={scale:.2}");
            theme::apply(&cc.egui_ctx, scale);
            Ok(Box::new(app::App::new(taken, scale)) as Box<dyn eframe::App>)
        }
    };

    let first = eframe::run_native("ferromix", options.clone(), Box::new(make_builder()));

    if let Err(e) = first {
        log::warn!("first window attempt failed ({e}); retrying under XWayland");
        eprintln!("Wayland GL context failed — retrying via XWayland…");
        // Force the X11/XWayland path for this process, then retry once.
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        std::env::remove_var("WAYLAND_DISPLAY");

        match eframe::run_native("ferromix", options, Box::new(make_builder())) {
            Ok(()) => log::info!("GUI closed cleanly (XWayland)"),
            Err(e2) => {
                eprintln!("\n╔══════════════════════════════════════════════╗");
                eprintln!("║  GUI could not open on Wayland OR XWayland    ║");
                eprintln!("╚══════════════════════════════════════════════╝");
                eprintln!("wayland error: {e}");
                eprintln!("xwayland error: {e2}");
                eprintln!("\nPlease paste this whole block. Also try:");
                eprintln!("  LIBGL_ALWAYS_SOFTWARE=1 cargo run -p mixer-gui");
                std::process::exit(1);
            }
        }
    } else {
        log::info!("GUI closed cleanly");
    }
}
