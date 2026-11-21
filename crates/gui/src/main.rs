//! `cardea-gui`: the Cardea window, a client of the daemon's control socket.

mod app;
mod conn;
mod focus;
mod instance;
mod style;
mod ui;

use std::process::ExitCode;

const USAGE: &str = "usage: cardea-gui [--theme light|dark]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let forced = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => None,
        ["--theme", "light"] => Some(false),
        ["--theme", "dark"] => Some(true),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match instance::claim() {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("Cardea is already open: showing its window.");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("cardea-gui: {e}");
            return ExitCode::FAILURE;
        }
    }
    let result = iced::application(
        move || app::App::new(forced),
        app::App::update,
        app::App::view,
    )
    .default_font(ui::kit::font())
    .title(app::App::title)
    .theme(app::App::theme)
    .subscription(app::App::subscription)
    .window(iced::window::Settings {
        size: iced::Size::new(1100.0, 720.0),
        min_size: Some(iced::Size::new(800.0, 550.0)),
        // Matches `cardea.desktop`: Wayland compositors take the window icon from it.
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: "cardea".into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .run();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cardea-gui: {e}");
            ExitCode::FAILURE
        }
    }
}
