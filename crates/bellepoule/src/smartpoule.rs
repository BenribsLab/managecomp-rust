//! SmartPoule : liaison avec les téléphones des arbitres par le serveur
//! d'API de BellePoule Web (relais sur Internet : BellePoule et les
//! téléphones n'ont pas besoin d'être sur le même réseau).
//!
//! Toutes les 3 secondes, si le serveur et la clé d'API du club sont
//! réglés : BellePoule dépose un instantané (arbitres et leurs clés, matchs à
//! arbitrer, feuilles) puis relève les scores saisis sur les téléphones, les
//! applique et rend compte du résultat.
//!
//! Réglages (`Tournament/…` de la configuration) : `SmartPoule/server`,
//! `SmartPoule/api_key`, `SmartPoule/club` (identifiant renvoyé par le
//! serveur), `SmartPoule/admin_key` (QR code de l'administrateur).

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use bp_core::i18n::tr;
use bp_gtk::dialogs;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use serde_json::{json, Value};

use crate::app::App;
use crate::contest_page::ContestPage;

/// Réglages de la liaison.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub server: String,
    pub api_key: String,
    pub club: String,
    pub admin_key: String,
}

impl Settings {
    pub fn load(app: &App) -> Settings {
        let t = app.tournament.borrow();
        let get = |k: &str| t.config.get_string("SmartPoule", k).unwrap_or_default();
        let s = Settings { server: get("server"), api_key: get("api_key"), club: get("club"), admin_key: get("admin_key") };
        // Les impressions (feuilles de poule et de match) en tirent leurs QR codes.
        SHEET_QR.with(|q| *q.borrow_mut() = (s.configured() && !s.club.is_empty()).then(|| s.clone()));
        s
    }

    fn save(&self, app: &App) {
        let mut t = app.tournament.borrow_mut();
        t.config.set_string("SmartPoule", "server", &self.server);
        t.config.set_string("SmartPoule", "api_key", &self.api_key);
        t.config.set_string("SmartPoule", "club", &self.club);
        t.config.set_string("SmartPoule", "admin_key", &self.admin_key);
        let _ = t.save_config();
    }

    pub(crate) fn base(&self) -> String {
        self.server.trim().trim_end_matches('/').to_string()
    }

    pub fn configured(&self) -> bool {
        !self.server.trim().is_empty() && !self.api_key.trim().is_empty()
    }

    /// Page ManageComp Arbitre (`https://arbitre.<domaine>/` quand le
    /// serveur est `https://api.<domaine>`).
    fn arbitre_base(&self) -> String {
        let base = self.base();
        match base.split_once("://") {
            Some((scheme, host)) if host.starts_with("api.") && !host.contains('/') => format!("{scheme}://arbitre.{}/", &host[4..]),
            _ => format!("{base}/smartpoule/"),
        }
    }

    /// QR code personnel d'un arbitre (`reference = 0` : administrateur), sans PIN.
    pub fn qr_text(&self, reference: u32, key: &str) -> String {
        format!("{}?c={}&r={reference}&k={key}", self.arbitre_base(), self.club)
    }

    /// QR code d'une feuille (poule, match) : il faut ensuite le PIN d'un
    /// arbitre de la feuille.
    pub fn sheet_qr_text(&self, token: &str) -> String {
        format!("{}?c={}&s={token}", self.arbitre_base(), self.club)
    }
}

thread_local! {
    /// Réglages connus de la liaison, pour les QR codes des impressions.
    static SHEET_QR: RefCell<Option<Settings>> = const { RefCell::new(None) };
}

/// QR code à imprimer sur la feuille d'une poule ou d'un match (liaison
/// réglée et club connu).
pub fn sheet_qr(token: &str) -> Option<String> {
    SHEET_QR.with(|q| q.borrow().as_ref().map(|s| s.sheet_qr_text(token)))
}

impl Settings {
}

/// Résultats renvoyés par le fil réseau.
enum Incoming {
    Club(String),
    Pending(Vec<Value>),
    Status(String),
    Security(Value),
}

pub struct SmartPoule {
    app: Weak<App>,
    last_state: RefCell<String>,
    busy: Arc<AtomicBool>,
    tx: Sender<Incoming>,
    rx: Receiver<Incoming>,
    pub status: RefCell<String>,
    /// Feuilles bloquées (PIN), téléphones reconnus (`/api/smartpoule/security`).
    pub security: RefCell<Value>,
    /// Feuilles bloquées déjà signalées.
    alerted: RefCell<Vec<String>>,
    /// Signalements des arbitres déjà annoncés.
    seen_reports: RefCell<Vec<u64>>,
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build()
}

impl SmartPoule {
    pub fn start(app: &Rc<App>) -> Rc<SmartPoule> {
        let (tx, rx) = channel();
        let sp = Rc::new(SmartPoule {
            app: Rc::downgrade(app),
            last_state: RefCell::new(String::new()),
            busy: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            status: RefCell::new(tr("Not configured")),
            security: RefCell::new(Value::Null),
            alerted: RefCell::new(Vec::new()),
            seen_reports: RefCell::new(Vec::new()),
        });
        let weak = Rc::downgrade(&sp);
        glib::timeout_add_local(Duration::from_secs(3), move || match weak.upgrade() {
            Some(sp) => {
                sp.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        sp
    }

    /// Un cycle : résultats reçus, puis nouvel échange si le précédent est fini.
    fn tick(&self) {
        let Some(app) = self.app.upgrade() else { return };
        while let Ok(incoming) = self.rx.try_recv() {
            match incoming {
                Incoming::Club(club) => {
                    let mut s = Settings::load(&app);
                    s.club = club;
                    s.save(&app);
                }
                Incoming::Pending(list) => self.apply(&app, list),
                Incoming::Status(text) => {
                    let locked = self.security.borrow().get("alerts").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
                    *self.status.borrow_mut() = if locked > 0 { format!("{text} — ⚠ {locked} {}", tr("blocked sheet(s)")) } else { text };
                }
                Incoming::Security(v) => self.on_security(&app, v),
            }
        }
        let mut settings = Settings::load(&app);
        if !settings.configured() {
            *self.status.borrow_mut() = tr("Not configured");
            return;
        }
        if settings.admin_key.is_empty() {
            settings.admin_key = bp_net::flash_code::get_key256();
            settings.save(&app);
        }
        // Épreuves publiées (compete.managecomp.org) : mises à jour en direct.
        crate::publish::push_updates(&app, &settings);
        if self.busy.load(Ordering::SeqCst) || app.tournament.try_borrow_mut().is_err() {
            return;
        }
        let state = self.snapshot(&app, &settings).to_string();
        let send_state = *self.last_state.borrow() != state;
        if send_state {
            *self.last_state.borrow_mut() = state.clone();
        }
        self.busy.store(true, Ordering::SeqCst);
        let (busy, tx) = (self.busy.clone(), self.tx.clone());
        let need_club = settings.club.is_empty();
        std::thread::spawn(move || {
            let base = settings.base();
            let key = settings.api_key.trim().to_string();
            let agent = agent();
            let result = (|| -> Result<(), String> {
                if need_club {
                    let me: Value = agent.get(&format!("{base}/api/me")).set("X-Api-Key", &key).call().map_err(describe)?.into_json().map_err(|e| e.to_string())?;
                    if let Some(id) = me.get("id").and_then(Value::as_str) {
                        let _ = tx.send(Incoming::Club(id.to_string()));
                    }
                }
                if send_state {
                    agent
                        .put(&format!("{base}/api/smartpoule/state"))
                        .set("X-Api-Key", &key)
                        .set("Content-Type", "application/json")
                        .send_string(&state)
                        .map_err(describe)?;
                }
                let pending: Value = agent.get(&format!("{base}/api/smartpoule/pending")).set("X-Api-Key", &key).call().map_err(describe)?.into_json().map_err(|e| e.to_string())?;
                if let Some(list) = pending.as_array().filter(|l| !l.is_empty()) {
                    let _ = tx.send(Incoming::Pending(list.clone()));
                }
                if let Ok(security) = agent.get(&format!("{base}/api/smartpoule/security")).set("X-Api-Key", &key).call().map_err(describe).and_then(|r| r.into_json::<Value>().map_err(|e| e.to_string())) {
                    let _ = tx.send(Incoming::Security(security));
                }
                Ok(())
            })();
            let _ = tx.send(Incoming::Status(match result {
                Ok(()) => tr("Connected"),
                Err(e) => format!("{} : {e}", tr("Connection error")),
            }));
            busy.store(false, Ordering::SeqCst);
        });
        // Un échec d'envoi de l'instantané le fera renvoyer.
        if !send_state {
            return;
        }
    }

    /// Alertes du serveur : une feuille bloquée (trop de PIN faux) est
    /// signalée une fois.
    fn on_security(&self, app: &Rc<App>, v: Value) {
        let fresh: Vec<String> = v
            .get("alerts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|a| !self.alerted.borrow().iter().any(|t| Some(t.as_str()) == a.get("token").and_then(Value::as_str)))
            .map(|a| a.get("name").and_then(Value::as_str).unwrap_or("").to_string())
            .collect();
        *self.alerted.borrow_mut() = v.get("alerts").and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.get("token").and_then(Value::as_str).map(str::to_string)).collect();
        // Signalements des arbitres : puce rouge du menu, message pour les nouveaux.
        let open: Vec<Value> = v.get("reports").and_then(Value::as_array).into_iter().flatten().filter(|r| !r.get("done").and_then(Value::as_bool).unwrap_or(false)).cloned().collect();
        if let Some(shell) = app.shell.borrow().as_ref() {
            shell.set_reports(open.len());
        }
        let fresh_reports: Vec<&Value> = open.iter().filter(|r| !self.seen_reports.borrow().contains(&r.get("id").and_then(Value::as_u64).unwrap_or(0))).collect();
        // Son : l'appel du Directoire technique passe avant l'absence.
        if fresh_reports.iter().any(|r| r.get("kind").and_then(Value::as_str) == Some("call")) {
            crate::sound::play(crate::sound::Alert::Call);
        } else if !fresh_reports.is_empty() {
            crate::sound::play(crate::sound::Alert::Absence);
        }
        let new_reports: Vec<String> = fresh_reports
            .iter()
            .map(|r| {
                let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                format!("{} ({} · {})", text("message"), text("sheet"), text("referee"))
            })
            .collect();
        self.seen_reports.borrow_mut().extend(open.iter().filter_map(|r| r.get("id").and_then(Value::as_u64)));
        if !new_reports.is_empty() {
            let window = app.window.clone();
            glib::idle_add_local_once(move || {
                dialogs::info(Some(&window), &format!("🔴 {}", tr("Referee report")), Some(&format!("{}\n\n{}", new_reports.join("\n"), tr("Mark it as handled in BellePoule2D > Referees on phones."))));
            });
        }
        *self.security.borrow_mut() = v;
        if !fresh.is_empty() {
            let window = app.window.clone();
            glib::idle_add_local_once(move || {
                dialogs::info(
                    Some(&window),
                    &format!("⚠ {}", tr("Sheet blocked: too many wrong PIN codes")),
                    Some(&format!("{}\n\n{}", fresh.join("\n"), tr("Unlock it in BellePoule2D > Referees on phones."))),
                );
            });
        }
    }

    /// Requête au serveur (clé d'API du club) dans un fil, sans attendre.
    pub fn post(app: &App, path: &str, body: Value) {
        let s = Settings::load(app);
        if !s.configured() {
            return;
        }
        let url = format!("{}{path}", s.base());
        std::thread::spawn(move || {
            let _ = agent().post(&url).set("X-Api-Key", s.api_key.trim()).set("Content-Type", "application/json").send_string(&body.to_string());
        });
    }

    /// Instantané : arbitres et leurs clés, matchs, feuilles.
    fn snapshot(&self, app: &Rc<App>, settings: &Settings) -> Value {
        let pages: Vec<Rc<ContestPage>> = app.pages.borrow().clone();
        let mut referees: Vec<Value> = Vec::new();
        let mut jobs: Vec<Value> = Vec::new();
        let mut sheets = serde_json::Map::new();
        for page in &pages {
            let Some(()) = page.try_with_contest(|c| {
                c.ensure_referee_keys();
                for (r, key) in c.referee_passwords() {
                    if !referees.iter().any(|x| x["reference"] == json!(r)) {
                        let name = c.referees.get(r).map(|p| format!("{} {}", p.name(), p.first_name())).unwrap_or_default();
                        referees.push(json!({ "reference": r, "key": key, "name": name }));
                    }
                }
                for job in c.smartpoule_jobs(None) {
                    if let Some(token) = job.get("token").and_then(Value::as_str) {
                        if let Some(sheet) = c.smartpoule_sheet(token, None) {
                            sheets.insert(token.to_string(), sheet);
                        }
                    }
                    jobs.push(job);
                }
            }) else {
                continue;
            };
        }
        json!({ "admin_key": settings.admin_key, "referees": referees, "jobs": jobs, "sheets": sheets })
    }

    /// Applique les scores reçus puis rend compte au serveur.
    fn apply(&self, app: &Rc<App>, list: Vec<Value>) {
        let pages: Vec<Rc<ContestPage>> = app.pages.borrow().clone();
        let mut results = Vec::new();
        for s in list {
            let id = s.get("id").and_then(Value::as_u64).unwrap_or(0);
            let token = s.get("token").and_then(Value::as_str).unwrap_or("").to_string();
            let bout = s.get("bout").and_then(Value::as_u64).unwrap_or(0) as usize;
            let values: Vec<String> = s.get("values").and_then(Value::as_array).map(|v| v.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
            let contest: u32 = token.split('~').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
            let page = pages.iter().find(|p| p.with_contest(|c| c.net_id == contest).unwrap_or(false));
            let officiating = s.get("officiating").filter(|v| !v.is_null()).cloned();
            let relay = s.get("relay").filter(|v| !v.is_null()).cloned();
            let outcome = match page {
                Some(page) => {
                    let r = page.with_contest(|c| -> Result<(), String> {
                        if let Some(state) = officiating {
                            let state = serde_json::from_value(state).map_err(|_| "journal d'arbitrage invalide")?;
                            c.smartpoule_apply_officiating(&token, bout, state)?;
                        }
                        if let Some(log) = relay.clone() {
                            let log = serde_json::from_value(log).map_err(|_| "journal des relais invalide")?;
                            c.smartpoule_apply_relay(&token, bout, log)?;
                        }
                        if values.len() == 2 { c.smartpoule_apply_score(&token, bout, [values[0].clone(), values[1].clone()])?; }
                        else if s.get("officiating").is_none_or(|v| v.is_null()) && relay.is_none() { return Err("saisie incomplète".into()); }
                        Ok(())
                    }).unwrap_or(Err("compétition fermée".into()));
                    if r.is_ok() {
                        page.mark_dirty();
                        if let Some(view) = page.schedule.borrow().clone() {
                            view.refresh_screens();
                        }
                    }
                    r
                }
                None => Err("compétition non ouverte dans ManageComp".into()),
            };
            results.push(match outcome {
                Ok(()) => json!({ "id": id, "ok": true }),
                Err(e) => json!({ "id": id, "ok": false, "error": e }),
            });
        }
        let settings = Settings::load(app);
        let body = json!({ "results": results }).to_string();
        // Les feuilles changent : nouvel instantané au prochain cycle.
        self.last_state.borrow_mut().clear();
        std::thread::spawn(move || {
            let _ = agent()
                .post(&format!("{}/api/smartpoule/results", settings.base()))
                .set("X-Api-Key", settings.api_key.trim())
                .set("Content-Type", "application/json")
                .send_string(&body);
        });
    }
}

pub(crate) fn describe(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => tr("API key refused"),
        ureq::Error::Status(code, r) => format!("HTTP {code} {}", r.into_string().unwrap_or_default()),
        ureq::Error::Transport(t) => t.to_string(),
    }
}


// ------------------------------------------------------------------ dialogue

/// Adresse des écrans de piste : sous-domaine `live.` quand le serveur est
/// `https://api.<domaine>` (https://live.managecomp.org/?c=…), sinon `/live/`.
fn live_link(server: &str, club: &str) -> String {
    let server = server.trim().trim_end_matches('/');
    match server.split_once("://") {
        Some((scheme, host)) if host.starts_with("api.") && !host.contains('/') => format!("{scheme}://live.{}/?c={club}", &host[4..]),
        _ => format!("{server}/live/?c={club}"),
    }
}

/// Heure locale (« 14:32 ») d'un instant du serveur (secondes depuis 1970).
fn clock(secs: &str) -> String {
    use chrono::TimeZone;
    secs.parse::<i64>().ok().and_then(|s| chrono::Local.timestamp_opt(s, 0).single()).map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

pub(crate) fn qr_texture(text: &str) -> Option<gtk::gdk::Texture> {
    let png = bp_net::flash_code::QrMatrix::encode(text)?.to_image(4).to_png();
    gtk::gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)).ok()
}

/// Menu BellePoule2D > SmartPoule… : réglages du serveur, QR codes de
/// l'administrateur et des arbitres, impression des QR codes.
pub fn show_dialog(app: &Rc<App>) {
    let settings = Settings::load(app);
    let dialog = gtk::Dialog::with_buttons(
        Some("Arbitres sur téléphone"),
        Some(&app.window),
        gtk::DialogFlags::MODAL,
        &[(&tr("_Close"), gtk::ResponseType::Close)],
    );
    dialog.set_default_size(720, 640);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_start(14);
    content.set_margin_end(14);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    let intro = gtk::Label::new(Some(&tr("The referees' phones (ManageComp Arbitre app) exchange through your ManageComp server: no need to be on the same network.")));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    content.append(&intro);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(10);
    let server = gtk::Entry::new();
    server.set_placeholder_text(Some("https://bellepoule.monclub.fr"));
    server.set_text(&settings.server);
    server.set_hexpand(true);
    let key = gtk::PasswordEntry::new();
    key.set_show_peek_icon(true);
    key.set_text(&settings.api_key);
    for (row, (label, widget)) in [(tr("Server address"), server.upcast_ref::<gtk::Widget>()), (tr("Club API key"), key.upcast_ref())].into_iter().enumerate() {
        let l = gtk::Label::new(Some(&label));
        l.set_xalign(1.0);
        grid.attach(&l, 0, row as i32, 1, 1);
        grid.attach(widget, 1, row as i32, 1, 1);
    }
    let save = gtk::Button::with_label(&tr("Save and connect"));
    grid.attach(&save, 1, 2, 1, 1);
    content.append(&grid);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    let current = app.smartpoule.borrow().as_ref().map(|s| s.status.borrow().clone()).unwrap_or_default();
    status.set_text(&format!("{} : {current}", tr("Status")));
    content.append(&status);

    // Écrans de piste : LivePoule relit les scores en direct sur le serveur.
    // L'adresse suit les réglages (recalculée après « Enregistrer et connecter »).
    let live_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let live_entry = gtk::Entry::new();
    let live_hint = gtk::Label::new(Some(&tr("Start each screen with: managecomp-live --live <this address> --piste <number>. The same address opened in a browser shows the live score of a piste.")));
    {
        live_row.append(&gtk::Label::new(Some(&tr("Piste screens (ManageComp Live)"))));
        live_entry.set_editable(false);
        live_entry.set_hexpand(true);
        live_row.append(&live_entry);
        let copy = gtk::Button::with_label(&tr("Copy"));
        let entry = live_entry.clone();
        copy.connect_clicked(move |b| b.clipboard().set_text(&entry.text()));
        live_row.append(&copy);
        content.append(&live_row);
        live_hint.set_wrap(true);
        live_hint.set_xalign(0.0);
        live_hint.add_css_class("dim-label");
        content.append(&live_hint);
    }
    let update_live = {
        let (app, row, entry, hint) = (app.clone(), live_row.clone(), live_entry.clone(), live_hint.clone());
        move || {
            let s = Settings::load(&app);
            let ready = !s.server.trim().is_empty() && !s.club.is_empty();
            if ready {
                entry.set_text(&live_link(&s.server, &s.club));
            }
            row.set_visible(ready);
            hint.set_visible(ready);
        }
    };
    update_live();

    let codes = gtk::FlowBox::new();
    codes.set_selection_mode(gtk::SelectionMode::None);
    codes.set_max_children_per_line(4);
    codes.set_row_spacing(10);
    codes.set_column_spacing(10);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_child(Some(&codes));
    // Feuilles bloquées (trop de PIN faux) : à débloquer.
    let alerts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    content.append(&alerts);
    let fill_alerts = {
        let app = app.clone();
        let alerts = alerts.clone();
        move || {
            while let Some(child) = alerts.first_child() {
                alerts.remove(&child);
            }
            let security = app.smartpoule.borrow().as_ref().map(|s| s.security.borrow().clone()).unwrap_or(Value::Null);
            // Signalements des arbitres en cours.
            for r in security.get("reports").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter(|r| !r.get("done").and_then(Value::as_bool).unwrap_or(false)) {
                let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let l = gtk::Label::new(None);
                l.set_markup(&format!(
                    "🔴 <b>{}</b>\n<small>{}</small>",
                    glib::markup_escape_text(&text("message")),
                    glib::markup_escape_text(&[text("sheet"), text("competition"), format!("{} {} · {}", tr("reported by"), text("referee"), clock(&text("at")))].into_iter().filter(|x| !x.trim().is_empty()).collect::<Vec<_>>().join(" · "))
                ));
                l.set_hexpand(true);
                l.set_xalign(0.0);
                l.set_wrap(true);
                row.append(&l);
                let handled = gtk::Button::with_label(&tr("Handled"));
                let id = r.get("id").and_then(Value::as_u64).unwrap_or(0);
                let (app, row2) = (app.clone(), row.clone());
                handled.connect_clicked(move |_| {
                    SmartPoule::post(&app, "/api/smartpoule/security", json!({ "action": "report_done", "id": id, "done": true }));
                    row2.set_visible(false);
                });
                row.append(&handled);
                alerts.append(&row);
            }
            for a in security.get("alerts").and_then(Value::as_array).cloned().unwrap_or_default() {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let l = gtk::Label::new(None);
                l.set_markup(&format!(
                    "<b>⚠ {}</b> {} ({})",
                    tr("Blocked sheet"),
                    glib::markup_escape_text(a.get("name").and_then(Value::as_str).unwrap_or("")),
                    glib::markup_escape_text(&clock(a.get("since").and_then(Value::as_str).unwrap_or("")))
                ));
                l.set_hexpand(true);
                l.set_xalign(0.0);
                l.set_wrap(true);
                row.append(&l);
                let unlock = gtk::Button::with_label(&tr("Unlock"));
                let token = a.get("token").and_then(Value::as_str).unwrap_or("").to_string();
                let (app, row2) = (app.clone(), row.clone());
                unlock.connect_clicked(move |_| {
                    SmartPoule::post(&app, "/api/smartpoule/security", json!({ "action": "unlock", "token": token }));
                    row2.set_visible(false);
                });
                row.append(&unlock);
                alerts.append(&row);
            }
        }
    };
    fill_alerts();

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let print = gtk::Button::with_label(&tr("Print the QR codes"));
    let personal = gtk::Button::with_label(&tr("Personal sheets"));
    personal.set_tooltip_text(Some(&tr("One page per referee: personal QR code (no PIN) and PIN code")));
    let pins = gtk::Button::with_label(&tr("PIN list"));
    buttons.append(&print);
    buttons.append(&personal);
    buttons.append(&pins);
    content.append(&buttons);
    let pin_hint = gtk::Label::new(Some(&tr("Pool and match sheets carry a QR code too: the referee scans it, then enters the PIN (once for the competition). After 20 wrong PINs the sheet pauses 5 minutes, after 20 more it is blocked until you unlock it.")));
    pin_hint.set_wrap(true);
    pin_hint.set_xalign(0.0);
    pin_hint.add_css_class("dim-label");
    content.append(&pin_hint);
    content.append(&scroller);

    // Cartes : administrateur, puis chaque arbitre des compétitions ouvertes.
    let cards = {
        let app = app.clone();
        move || -> Vec<Card> {
            let s = Settings::load(&app);
            if !s.configured() || s.club.is_empty() {
                return Vec::new();
            }
            let mut out = vec![Card { reference: 0, name: tr("Administrator"), detail: tr("all the bouts"), text: s.qr_text(0, &s.admin_key), pin: None }];
            let pages: Vec<Rc<ContestPage>> = app.pages.borrow().clone();
            for page in pages {
                let list = page
                    .with_contest(|c| {
                        c.ensure_referee_keys();
                        c.referee_passwords()
                            .into_iter()
                            .map(|(r, k)| {
                                let p = c.referees.get(r);
                                let name = p.map(|p| format!("{} {}", p.name(), p.first_name())).unwrap_or_default();
                                let club = p.and_then(|p| p.str_attr("club").map(str::to_string)).unwrap_or_default();
                                Card { reference: r, name, detail: club, text: s.qr_text(r, &k), pin: Some(bp_core::smartpoule::referee_pin(&k)) }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for item in list {
                    if !out.iter().any(|x| x.text == item.text) {
                        out.push(item);
                    }
                }
            }
            out
        }
    };
    let fill: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let fill_now = {
        let fill = fill.clone();
        move || {
            let f = fill.borrow().clone();
            if let Some(f) = f {
                f();
            }
        }
    };
    *fill.borrow_mut() = Some(Rc::new({
        let codes = codes.clone();
        let cards = cards.clone();
        let app = app.clone();
        let fill_now = fill_now.clone();
        move || {
            while let Some(child) = codes.first_child() {
                codes.remove(&child);
            }
            let list = cards();
            if list.is_empty() {
                let l = gtk::Label::new(Some(&tr("Enter the server address and the club API key, then save: the QR codes appear once connected.")));
                l.set_wrap(true);
                codes.insert(&l, -1);
            }
            for item in list {
                let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
                card.add_css_class("bp-recent-card");
                if let Some(t) = qr_texture(&item.text) {
                    let image = gtk::Image::from_paintable(Some(&t));
                    image.set_pixel_size(150);
                    card.append(&image);
                }
                let n = gtk::Label::new(None);
                n.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&item.name)));
                card.append(&n);
                let d = gtk::Label::new(Some(&item.detail));
                d.add_css_class("bp-muted");
                card.append(&d);
                if let Some(pin) = &item.pin {
                    let p = gtk::Label::new(None);
                    p.set_markup(&format!("PIN : <b>{pin}</b>"));
                    card.append(&p);
                    let renew = gtk::Button::with_label(&tr("New code"));
                    renew.set_tooltip_text(Some(&tr("New personal QR code and new PIN (if a PIN has leaked): the referee's phones must reconnect")));
                    let (app, fill_now, reference, name) = (app.clone(), fill_now.clone(), item.reference, item.name.clone());
                    renew.connect_clicked(move |_| {
                        if !dialogs::confirm(Some(&app.window), &format!("{} {name} ?", tr("New code for")), Some(&tr("The current personal QR code and PIN will no longer work. Remember to give the referee the new sheet.")), None) {
                            return;
                        }
                        renew_code(&app, reference);
                        fill_now();
                    });
                    card.append(&renew);
                }
                codes.insert(&card, -1);
            }
        }
    }));
    let fill = fill_now;
    fill();
    {
        let app = app.clone();
        let fill = fill.clone();
        let status = status.clone();
        save.connect_clicked(move |_| {
            let mut s = Settings::load(&app);
            let new_server = server.text().trim().to_string();
            let new_key = key.text().trim().to_string();
            if new_server != s.server || new_key != s.api_key {
                s.club.clear();
            }
            s.server = new_server;
            s.api_key = new_key;
            s.save(&app);
            if let Some(sp) = app.smartpoule.borrow().as_ref() {
                sp.last_state.borrow_mut().clear();
            }
            status.set_text(&format!("{} : {}", tr("Status"), tr("connecting…")));
            // Laisse le temps de récupérer l'identifiant du club.
            let (app2, fill, status, update_live) = (app.clone(), fill.clone(), status.clone(), update_live.clone());
            update_live();
            glib::timeout_add_local_once(Duration::from_secs(4), move || {
                let current = app2.smartpoule.borrow().as_ref().map(|s| s.status.borrow().clone()).unwrap_or_default();
                status.set_text(&format!("{} : {current}", tr("Status")));
                fill();
                update_live();
            });
        });
    }
    for (button, mode) in [(&print, PrintMode::Grid), (&personal, PrintMode::Personal), (&pins, PrintMode::Pins)] {
        let (app, cards) = (app.clone(), cards.clone());
        button.connect_clicked(move |_| {
            let mut list = cards();
            if mode != PrintMode::Grid {
                list.retain(|c| c.pin.is_some());
            }
            if list.is_empty() {
                return;
            }
            let club = Settings::load(&app).club;
            let result = bp_gtk::print::run_document(Some(&app.window), "QR codes des arbitres", move || Box::new(QrPrint { cards: list.clone(), mode, club: club.clone() }));
            if let Err(e) = result {
                dialogs::error(Some(&app.window), &e.to_string());
            }
        });
    }
    dialog.content_area().append(&content);
    dialogs::run_dialog(&dialog);
    dialog.destroy();
}

/// QR code personnel d'un arbitre (ou de l'administrateur) et son PIN.
#[derive(Clone)]
struct Card {
    reference: u32,
    name: String,
    detail: String,
    text: String,
    pin: Option<String>,
}

/// Nouveau code d'un arbitre dans les compétitions ouvertes : nouveau QR
/// code personnel et nouveau PIN ; ses téléphones devront se reconnecter.
fn renew_code(app: &Rc<App>, reference: u32) {
    let key = bp_core::smartpoule::new_referee_key();
    let pages: Vec<Rc<ContestPage>> = app.pages.borrow().clone();
    for page in pages {
        let changed = page
            .with_contest(|c| {
                let p = c.referees.get_mut(reference)?;
                p.set_str(&bp_core::player::AttrId::global("password"), &key);
                c.dirty = true;
                Some(())
            })
            .flatten()
            .is_some();
        if changed {
            page.mark_dirty();
        }
    }
    if let Some(sp) = app.smartpoule.borrow().as_ref() {
        sp.last_state.borrow_mut().clear();
    }
    SmartPoule::post(app, "/api/smartpoule/security", json!({ "action": "logout", "referee": reference }));
}

#[derive(Clone, Copy, PartialEq)]
enum PrintMode {
    /// 12 QR codes par page.
    Grid,
    /// Une feuille par arbitre : QR code personnel et PIN.
    Personal,
    /// Liste des PIN (organisation).
    Pins,
}

/// Impression des QR codes.
struct QrPrint {
    cards: Vec<Card>,
    mode: PrintMode,
    club: String,
}

fn draw_qr(cr: &cairo::Context, text: &str, x: f64, y: f64, size: f64) {
    if let Some(m) = bp_net::flash_code::QrMatrix::encode(text) {
        let module = size / m.width as f64;
        cr.set_source_rgb(0.0, 0.0, 0.0);
        for yy in 0..m.width {
            for xx in 0..m.width {
                if m.is_dark(xx, yy) {
                    cr.rectangle(x + xx as f64 * module, y + yy as f64 * module, module + 0.05, module + 0.05);
                }
            }
        }
        let _ = cr.fill();
    }
}

fn centered(cr: &cairo::Context, text: &str, font: &str, x: f64, y: f64, width: f64) -> f64 {
    let layout = pangocairo::functions::create_layout(cr);
    layout.set_font_description(Some(&pango::FontDescription::from_string(font)));
    layout.set_width((width * pango::SCALE as f64) as i32);
    layout.set_wrap(pango::WrapMode::WordChar);
    layout.set_alignment(pango::Alignment::Center);
    layout.set_markup(text);
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, &layout);
    layout.pixel_size().1 as f64
}

impl QrPrint {
    fn draw_personal(&self, card: &Card, cr: &cairo::Context, width: f64, height: f64) {
        let esc = |s: &str| glib::markup_escape_text(s).to_string();
        let mut y = height * 0.04;
        y += centered(cr, &format!("<b>{}</b>", esc(&card.name)), "Sans 24", 0.0, y, width) + 6.0;
        y += centered(cr, &esc(&card.detail), "Sans 13", 0.0, y, width) + height * 0.03;
        let size = (width * 0.55).min(height * 0.42);
        draw_qr(cr, &card.text, (width - size) / 2.0, y, size);
        y += size + height * 0.03;
        y += centered(cr, &esc(&tr("Scan this QR code with your phone's camera: your pools and bouts appear, without password. This sheet is personal, keep it.")), "Sans 11", width * 0.1, y, width * 0.8) + height * 0.03;
        if let Some(pin) = &card.pin {
            y += centered(cr, &format!("{} <b>{pin}</b>", esc(&tr("Your PIN code:"))), "Sans 20", 0.0, y, width) + height * 0.02;
            centered(cr, &esc(&tr("Pool and match sheets carry a QR code too: scan it, then enter this PIN code (once for the competition).")), "Sans 11", width * 0.1, y, width * 0.8);
        }
    }

    fn draw_pins(&self, page: usize, cr: &cairo::Context, width: f64) {
        let esc = |s: &str| glib::markup_escape_text(s).to_string();
        let mut y = 0.0;
        if page == 0 {
            y += centered(cr, &format!("<b>{}</b> — {}", esc(&tr("Referees' PIN codes")), esc(&self.club)), "Sans 16", 0.0, y, width) + 4.0;
            y += centered(cr, &esc(&tr("Confidential: for the organisers.")), "Sans 9", 0.0, y, width) + 10.0;
        }
        for card in self.cards.iter().skip(page * 40).take(40) {
            let layout = pangocairo::functions::create_layout(cr);
            layout.set_font_description(Some(&pango::FontDescription::from_string("Sans 11")));
            layout.set_markup(&format!("{}  <span foreground=\"dimgrey\">{}</span>", esc(&card.name), esc(&card.detail)));
            cr.set_source_rgb(0.0, 0.0, 0.0);
            cr.move_to(10.0, y);
            pangocairo::functions::show_layout(cr, &layout);
            layout.set_markup(&format!("<b>{}</b>", card.pin.clone().unwrap_or_default()));
            cr.move_to(width - 80.0, y);
            pangocairo::functions::show_layout(cr, &layout);
            y += 18.0;
        }
    }
}

impl bp_gtk::print::PageSource for QrPrint {
    fn begin(&mut self, _width: f64, _height: f64) -> usize {
        match self.mode {
            PrintMode::Grid => self.cards.len().div_ceil(12).max(1),
            PrintMode::Personal => self.cards.len().max(1),
            PrintMode::Pins => self.cards.len().div_ceil(40).max(1),
        }
    }

    fn page_name(&self, _page: usize) -> String {
        "Arbitres sur téléphone".into()
    }

    fn draw(&mut self, page: usize, cr: &cairo::Context, width: f64, height: f64) {
        match self.mode {
            PrintMode::Personal => {
                if let Some(card) = self.cards.get(page) {
                    self.draw_personal(card, cr, width, height);
                }
                return;
            }
            PrintMode::Pins => return self.draw_pins(page, cr, width),
            PrintMode::Grid => {}
        }
        let (cols, rows) = (3usize, 4usize);
        let (cw, ch) = (width / cols as f64, height / rows as f64);
        for (i, card) in self.cards.iter().skip(page * 12).take(12).enumerate() {
            let (name, detail, text) = (&card.name, &match &card.pin { Some(pin) => format!("{} · PIN {pin}", card.detail), None => card.detail.clone() }, &card.text);
            let (x, y) = ((i % cols) as f64 * cw, (i / cols) as f64 * ch);
            cr.set_source_rgb(0.6, 0.6, 0.6);
            cr.set_line_width(0.5);
            cr.rectangle(x + 4.0, y + 4.0, cw - 8.0, ch - 8.0);
            let _ = cr.stroke();
            if let Some(m) = bp_net::flash_code::QrMatrix::encode(text) {
                let size = (cw.min(ch) - 50.0).max(20.0);
                let module = size / m.width as f64;
                let (ox, oy) = (x + (cw - size) / 2.0, y + 10.0);
                cr.set_source_rgb(0.0, 0.0, 0.0);
                for yy in 0..m.width {
                    for xx in 0..m.width {
                        if m.is_dark(xx, yy) {
                            cr.rectangle(ox + xx as f64 * module, oy + yy as f64 * module, module + 0.05, module + 0.05);
                        }
                    }
                }
                let _ = cr.fill();
            }
            let layout = pangocairo::functions::create_layout(cr);
            layout.set_font_description(Some(&pango::FontDescription::from_string("Sans Bold 10")));
            layout.set_text(name);
            layout.set_width(((cw - 16.0) * pango::SCALE as f64) as i32);
            layout.set_alignment(pango::Alignment::Center);
            cr.move_to(x + 8.0, y + ch - 36.0);
            pangocairo::functions::show_layout(cr, &layout);
            layout.set_font_description(Some(&pango::FontDescription::from_string("Sans 8")));
            layout.set_text(detail);
            cr.move_to(x + 8.0, y + ch - 20.0);
            pangocairo::functions::show_layout(cr, &layout);
        }
    }
}

use gtk4::pango;
