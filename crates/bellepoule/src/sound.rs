//! Sons des signalements des arbitres : deux notes pour l'absence d'un
//! tireur, trois notes montantes (plus pressantes) pour l'appel du
//! Directoire technique. Mêmes sons que la version web.
//!
//! Le son est fabriqué en mémoire (WAV) : joué par Windows (`PlaySoundW`),
//! ailleurs par `paplay`, `aplay` ou `afplay` ; à défaut, le bip du système.

use gtk4::prelude::*;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Absence,
    Call,
}

/// Notes (fréquence en Hz, durée en s), un silence entre chacune.
fn notes(alert: Alert) -> &'static [(f64, f64)] {
    match alert {
        Alert::Absence => &[(660.0, 0.18), (660.0, 0.18)],
        Alert::Call => &[(784.0, 0.15), (988.0, 0.15), (1319.0, 0.3)],
    }
}

const RATE: u32 = 22_050;
const GAP: f64 = 0.07;

/// WAV mono 16 bits des notes, avec attaque et extinction douces.
fn wav(alert: Alert) -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::new();
    for (i, (freq, duration)) in notes(alert).iter().enumerate() {
        if i > 0 {
            samples.extend(std::iter::repeat_n(0, (GAP * RATE as f64) as usize));
        }
        let n = (duration * RATE as f64) as usize;
        let fade = (0.012 * RATE as f64) as usize;
        for k in 0..n {
            let envelope = (k.min(n - k) as f64 / fade as f64).min(1.0);
            let value = (2.0 * std::f64::consts::PI * freq * k as f64 / RATE as f64).sin() * envelope * 0.5;
            samples.push((value * i16::MAX as f64) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Son prêt à jouer (fabriqué une fois, gardé pour la lecture asynchrone).
fn sound(alert: Alert) -> &'static [u8] {
    static ABSENCE: OnceLock<Vec<u8>> = OnceLock::new();
    static CALL: OnceLock<Vec<u8>> = OnceLock::new();
    match alert {
        Alert::Absence => ABSENCE.get_or_init(|| wav(Alert::Absence)),
        Alert::Call => CALL.get_or_init(|| wav(Alert::Call)),
    }
}

/// Joue le son d'un signalement, sans attendre.
pub fn play(alert: Alert) {
    let data = sound(alert);
    #[cfg(windows)]
    {
        use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
        // SAFETY : `data` est un WAV complet qui vit jusqu'à la fin du programme.
        let ok = unsafe { PlaySoundW(data.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
        if ok == 0 {
            beep();
        }
    }
    #[cfg(not(windows))]
    {
        let path = std::env::temp_dir().join(match alert {
            Alert::Absence => "managecomp-absence.wav",
            Alert::Call => "managecomp-appel.wav",
        });
        if std::fs::write(&path, data).is_err() {
            beep();
            return;
        }
        std::thread::spawn(move || {
            let played = ["paplay", "aplay", "afplay"].iter().any(|player| {
                std::process::Command::new(player)
                    .arg(&path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            });
            if !played {
                gtk4::glib::MainContext::default().invoke(beep);
            }
        });
    }
}

fn beep() {
    if let Some(display) = gtk4::gdk::Display::default() {
        display.beep();
    }
}
