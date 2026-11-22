//! `cardea-gui`: the Cardea window, a client of the daemon's control socket.

mod app;
mod conn;
mod focus;
mod instance;
mod linkwin;
mod style;
mod ui;

use std::process::ExitCode;

const USAGE: &str = "usage: cardea-gui [--theme light|dark] [--sandbox <id>] | --link <request id>";

/// The link window: its own instance beside the manager, one per waiting link (no single-instance lock).
fn link_window(id: &str) -> ExitCode {
    let Ok(id) = id.parse::<u64>() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let ask: cardea_proto::LinkAsk = match linkwin::call(cardea_proto::Method::LinkRequest { id })
        .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
    {
        Ok(a) => a,
        Err(e) => {
            eprintln!("cardea-gui: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = iced::application(
        move || linkwin::LinkWindow::new(ask.clone()),
        linkwin::LinkWindow::update,
        linkwin::LinkWindow::view,
    )
    .default_font(ui::kit::font())
    .title(linkwin::LinkWindow::title)
    .theme(linkwin::LinkWindow::theme)
    .subscription(linkwin::LinkWindow::subscription)
    .window(iced::window::Settings {
        size: iced::Size::new(580.0, 360.0),
        min_size: Some(iced::Size::new(420.0, 260.0)),
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

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut forced, mut sandbox) = (None, None);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next().as_deref()) {
            ("--theme", Some("light")) => forced = Some(false),
            ("--theme", Some("dark")) => forced = Some(true),
            ("--sandbox", Some(id)) => sandbox = Some(id.to_owned()),
            ("--link", Some(id)) => return link_window(id),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    match instance::claim(sandbox.as_deref()) {
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
        move || app::App::new(forced, sandbox.clone()),
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
