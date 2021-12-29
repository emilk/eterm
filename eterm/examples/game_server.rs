//! Example for something spinning fast (~60 Hz) and server
//! a eterm at the same time:

fn main() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .with_utc_timestamps()
        .init()
        .ok();

    let mut eterm_server = eterm::Server::new("0.0.0.0:8505").unwrap();
    eterm_server.set_minimum_update_interval(1.0);

    let mut demo_windows = egui_demo_lib::DemoWindows::default();

    loop {
        eterm_server
            .show(|egui_ctx: &egui::CtxRef, _client_id: eterm::ClientId| {
                egui::TopBottomPanel::bottom("game_server_info").show(egui_ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Server time:");
                        ui_clock(ui);
                    });
                });
                demo_windows.ui(egui_ctx);
            })
            .unwrap();

        std::thread::sleep(std::time::Duration::from_secs_f32(1.0 / 60.0));
    }
}

fn ui_clock(ui: &mut egui::Ui) {
    let seconds_since_midnight = seconds_since_midnight();

    ui.monospace(format!(
        "{:02}:{:02}:{:02}",
        (seconds_since_midnight % (24.0 * 60.0 * 60.0) / 3600.0).floor(),
        (seconds_since_midnight % (60.0 * 60.0) / 60.0).floor(),
        (seconds_since_midnight % 60.0).floor(),
    ));
}

fn seconds_since_midnight() -> f64 {
    use chrono::Timelike;
    let time = chrono::Local::now().time();
    time.num_seconds_from_midnight() as f64 + 1e-9 * (time.nanosecond() as f64)
}
