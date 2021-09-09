//! Example for something spinning fast (~60 Hz) and server
//! a eterm at the same time:

fn main() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()
        .ok();

    let mut eterm_server = eterm::Server::new("0.0.0.0:8505").unwrap();

    let mut frame_nr = 0;

    let mut demo_windows = egui_demo_lib::DemoWindows::default();

    loop {
        eterm_server
            .show(|egui_ctx: &egui::CtxRef, _client_id: eterm::ClientId| {
                egui::TopBottomPanel::bottom("game_server_info").show(egui_ctx, |ui| {
                    ui.label(format!("Server is on frame {}", frame_nr));
                });
                demo_windows.ui(egui_ctx);
            })
            .unwrap();

        frame_nr += 1;

        std::thread::sleep(std::time::Duration::from_secs_f32(1.0 / 60.0));
    }
}
