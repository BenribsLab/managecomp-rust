//! BellePoule : logiciel de gestion de compétitions d'escrime.
//!
//! Options : `bellepoule [fichier]` ouvre un fichier ; `--smoke-test` ouvre
//! la fenêtre principale, puis quitte au bout d'une seconde (vérification
//! automatique).

// Pas de console en version publiée (comme le C++ sous Windows).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#![allow(deprecated)]

mod app;
mod capture;
mod categories_tab;
mod points_tab;
mod publish;
mod season_window;
mod contest_page;
mod language;
mod loading;
mod network;
mod management;
mod publication;
mod schedule_view;
mod stages;
mod widgets;
mod shell;
mod smartpoule;
mod sound;
mod visuals;

use std::path::PathBuf;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    install_panic_log();
    env_logger::init();

    let mut file: Option<PathBuf> = None;
    let mut smoke_test = false;
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Captures de la documentation (`--doc-capture`).
    let capture = capture::Shot::from_args(&args);
    let mut skip = false;
    for arg in &args {
        if skip {
            skip = false;
            continue;
        }
        match arg.as_str() {
            "--smoke-test" => smoke_test = true,
            a if capture::Shot::takes_value(a) => skip = true,
            a if !a.starts_with('-') => file = Some(PathBuf::from(a)),
            _ => {}
        }
    }

    let application = gtk::Application::builder()
        .application_id("org.managecomp.ManageComp")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_startup(|_| bp_gtk::resources::install_app_icon());
    application.connect_activate(move |a| app::App::start(a, file.clone(), smoke_test, capture.clone()));
    application.run_with_args::<&str>(&[])
}

/// Conserver la panique même quand la console du lanceur se ferme.
fn install_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let directory = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("BellePoule")
            .join("logs");
        if std::fs::create_dir_all(&directory).is_ok() {
            let path = directory.join("bellepoule-panic.log");
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                let _ = writeln!(file, "\n--- ManageComp PID {} / Unix {} ---\n{}\n{}",
                    std::process::id(), timestamp, info, std::backtrace::Backtrace::force_capture());
                let _ = file.flush();
                eprintln!("ManageComp : rapport d'erreur dans {}", path.display());
            }
        }
        previous(info);
    }));
}
