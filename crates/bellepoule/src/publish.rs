//! Publication sur ManageComp (compete.managecomp.org) : page publique de
//! l'épreuve, suivie en direct par les parents et les tireurs (QR code),
//! gardée en archive. Plusieurs compétitions ouvertes peuvent être publiées
//! ensemble dans un **événement**.
//!
//! Passe par le serveur et la clé d'API du club réglés dans « Arbitres sur
//! téléphone » ; la compétition garde l'identifiant de sa publication (et de
//! son événement) dans son fichier. Les mises à jour partent toutes seules
//! (`SmartPoule::tick` → [`push_updates`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bp_core::i18n::tr;
use bp_gtk::dialogs;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use serde_json::{json, Value};

use crate::app::App;
use crate::contest_page::ContestPage;
use crate::smartpoule::{agent, describe, qr_texture, Settings};

/// Requête au serveur (clé d'API du club), dans un fil.
fn request(s: &Settings, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let url = format!("{}{path}", s.base());
    let req = agent().request(method, &url).set("X-Api-Key", s.api_key.trim());
    let resp = match body {
        Some(b) => req.set("Content-Type", "application/json").send_string(&b.to_string()),
        None => req.call(),
    }
    .map_err(describe)?;
    resp.into_json::<Value>().map_err(|e| e.to_string())
}

/// Lance `work` dans un fil, puis `done` sur le fil de l'interface.
fn in_background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    glib::spawn_future_local(async move {
        if let Ok(result) = gio::spawn_blocking(work).await {
            done(result);
        }
    });
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Bouton « Publier » : fenêtre de publication de la compétition active.
pub fn show_dialog(app: &Rc<App>) {
    let Some(page) = app.active_contest() else { return };
    let settings = Settings::load(app);
    if !settings.configured() {
        dialogs::info(
            Some(&app.window),
            &tr("Publish on ManageComp"),
            Some(&tr("Set the server address and the club API key first, in the Referees on phones window.")),
        );
        return;
    }
    let pages: Vec<Rc<ContestPage>> = app.pages.borrow().clone();
    // Compétition active en premier, puis les autres compétitions ouvertes.
    let mut ordered = vec![page.clone()];
    ordered.extend(pages.iter().filter(|p| !Rc::ptr_eq(p, &page)).cloned());
    let (public_id, public_event) = page.with_contest(|c| (c.public_id.clone(), c.public_event.clone())).unwrap_or_default();
    let first_name = pages.first().and_then(|p| p.with_contest(|c| c.name.clone())).unwrap_or_default();
    let mut categories: Vec<String> = Vec::new();
    for p in &pages {
        if let Some(cat) = p.with_contest(|c| c.category.display_image()) {
            if !cat.is_empty() && !categories.contains(&cat) {
                categories.push(cat);
            }
        }
    }

    let dialog = gtk::Dialog::with_buttons(Some(&tr("Publish on ManageComp")), Some(&app.window), gtk::DialogFlags::MODAL, &[(&tr("_Close"), gtk::ResponseType::Close)]);
    dialog.set_default_size(620, -1);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    for m in ["start", "end", "top", "bottom"] {
        match m {
            "start" => content.set_margin_start(14),
            "end" => content.set_margin_end(14),
            "top" => content.set_margin_top(12),
            _ => content.set_margin_bottom(12),
        }
    }
    let intro = gtk::Label::new(Some(&tr("A public page follows the event live (formula, fencers, pools, tables, ranking), with filters by name and club. Parents and fencers open it by scanning the QR code; it stays available after the competition.")));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    content.append(&intro);

    // Noms
    let names_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    names_box.append(&gtk::Label::new(Some(&tr("Fencers' names:"))));
    let full = gtk::CheckButton::with_label(&tr("Full names"));
    let short = gtk::CheckButton::with_label(&tr("First name and initial"));
    short.set_group(Some(&full));
    full.set_active(true);
    names_box.append(&full);
    names_box.append(&short);
    content.append(&names_box);
    let noindex = gtk::CheckButton::with_label(&tr("Hide from search engines"));
    noindex.set_tooltip_text(Some(&tr("The page stays available through its QR code and link, but does not show up on Google")));
    content.append(&noindex);

    // Événement : les compétitions ouvertes publiées ensemble.
    let grouped = gtk::CheckButton::with_label(&format!("{} ({})", tr("Group the open competitions in an event"), pages.len()));
    let ev_name = gtk::Entry::new();
    ev_name.set_text(&first_name);
    let ev_cats = gtk::Entry::new();
    ev_cats.set_text(&categories.join(", "));
    let ev_grid = gtk::Grid::new();
    ev_grid.set_row_spacing(6);
    ev_grid.set_column_spacing(10);
    for (row, (label, w)) in [(tr("Event name"), &ev_name), (tr("Categories"), &ev_cats)].into_iter().enumerate() {
        let l = gtk::Label::new(Some(&label));
        l.set_xalign(1.0);
        w.set_hexpand(true);
        ev_grid.attach(&l, 0, row as i32, 1, 1);
        ev_grid.attach(w, 1, row as i32, 1, 1);
    }
    if pages.len() >= 2 || public_event.is_some() {
        grouped.set_active(public_event.is_some() || public_id.is_none());
        content.append(&grouped);
        content.append(&ev_grid);
        ev_grid.set_sensitive(grouped.is_active());
        let g = ev_grid.clone();
        grouped.connect_toggled(move |b| g.set_sensitive(b.is_active()));
    }

    // État : lien, QR code, boutons
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let qr = gtk::Image::new();
    qr.set_pixel_size(150);
    let links = gtk::Box::new(gtk::Orientation::Vertical, 6);
    links.set_hexpand(true);
    let title = gtk::Label::new(None);
    title.set_xalign(0.0);
    let link = gtk::LinkButton::new("");
    link.set_halign(gtk::Align::Start);
    let short_label = gtk::Label::new(None);
    short_label.set_xalign(0.0);
    short_label.add_css_class("dim-label");
    short_label.set_selectable(true);
    let copy = gtk::Button::with_label(&tr("Copy the link"));
    copy.set_halign(gtk::Align::Start);
    links.append(&title);
    links.append(&link);
    links.append(&short_label);
    links.append(&copy);
    status.append(&qr);
    status.append(&links);
    status.set_visible(false);
    content.append(&status);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let publish = gtk::Button::with_label(&tr("Publish"));
    publish.add_css_class("suggested-action");
    let unpublish = gtk::Button::with_label(&tr("Unpublish"));
    unpublish.add_css_class("destructive-action");
    unpublish.set_visible(public_id.is_some());
    buttons.append(&publish);
    buttons.append(&unpublish);
    content.append(&buttons);
    let message = gtk::Label::new(None);
    message.set_xalign(0.0);
    message.set_wrap(true);
    content.append(&message);

    let current_url = Rc::new(RefCell::new(String::new()));
    {
        let url = current_url.clone();
        copy.connect_clicked(move |b| b.clipboard().set_text(&url.borrow()));
    }
    // Affiche l'état renvoyé par le serveur (épreuve, et son événement).
    let show_status = {
        let (status, qr, title, link, short_label, unpublish, publish, url, full, short, noindex) =
            (status.clone(), qr.clone(), title.clone(), link.clone(), short_label.clone(), unpublish.clone(), publish.clone(), current_url.clone(), full.clone(), short.clone(), noindex.clone());
        Rc::new(move |v: &Value| {
            let published = v.get("published").and_then(Value::as_bool).unwrap_or(false);
            status.set_visible(published);
            unpublish.set_visible(published);
            publish.set_label(&if published { tr("Publish again (options, event)") } else { tr("Publish") });
            if !published {
                return;
            }
            if text(v, "names") == "short" {
                short.set_active(true);
            } else {
                full.set_active(true);
            }
            noindex.set_active(v.get("noindex").and_then(Value::as_bool).unwrap_or(false));
            let event = v.get("event").filter(|e| !e.is_null());
            let (u, s) = match event {
                Some(e) => (text(e, "url"), text(e, "short_url")),
                None => (text(v, "url"), text(v, "short_url")),
            };
            title.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&if event.is_some() { tr("Event published ✔") } else { tr("Published ✔") })));
            link.set_uri(&u);
            link.set_label(&u);
            short_label.set_text(&format!("QR code : {s}"));
            if let Some(t) = qr_texture(&s) {
                qr.set_paintable(Some(&t));
            }
            *url.borrow_mut() = u;
        })
    };
    // État actuel de la publication.
    if let Some(id) = public_id.clone() {
        let s = settings.clone();
        let show = show_status.clone();
        let message = message.clone();
        in_background(move || request(&s, "GET", &format!("/api/publication/{id}"), None), move |r| match r {
            Ok(v) => show(&v),
            Err(e) => message.set_text(&e),
        });
    }

    {
        let (app, ordered, page, show, message) = (app.clone(), ordered.clone(), page.clone(), show_status.clone(), message.clone());
        let (grouped, ev_name, ev_cats, short, noindex) = (grouped.clone(), ev_name.clone(), ev_cats.clone(), short.clone(), noindex.clone());
        publish.connect_clicked(move |button| {
            let s = Settings::load(&app);
            let group = grouped.is_active() && grouped.parent().is_some();
            let targets: Vec<Rc<ContestPage>> = if group { ordered.clone() } else { vec![page.clone()] };
            // Événement existant d'une des compétitions, sinon nouveau.
            let existing = targets.iter().find_map(|p| p.with_contest(|c| c.public_event.clone()).flatten());
            let (date, location) = targets
                .iter()
                .filter_map(|p| p.with_contest(|c| (format!("{:04}-{:02}-{:02}", c.year, c.month, c.day), c.location.clone())))
                .min()
                .unwrap_or_default();
            let names = if short.is_active() { "short" } else { "full" };
            let hidden = noindex.is_active();
            let event_body = group.then(|| json!({ "id": existing, "name": ev_name.text().to_string(), "categories": ev_cats.text().to_string(), "date": date, "location": location, "noindex": hidden }));
            button.set_sensitive(false);
            message.set_text(&tr("Publishing…"));
            let (app2, targets2, page2, show2, message2, button2) = (app.clone(), targets.clone(), page.clone(), show.clone(), message.clone(), button.clone());
            // 1. Événement (identifiant gardé dans chaque compétition), 2. épreuves.
            let s2 = s.clone();
            in_background(
                move || event_body.map(|b| request(&s2, "PUT", "/api/publication/event", Some(b))).transpose(),
                move |event| {
                    let event_id = match event {
                        Ok(e) => e.map(|e| text(&e, "id")),
                        Err(e) => {
                            message2.set_text(&e);
                            button2.set_sensitive(true);
                            return;
                        }
                    };
                    let mut bodies = Vec::new();
                    for p in &targets2 {
                        let xml = p.with_contest(|c| {
                            c.public_event = event_id.clone();
                            c.to_xml()
                        });
                        if let Some(xml) = xml {
                            p.mark_dirty();
                            bodies.push(json!({ "xml": xml, "names": names, "noindex": hidden, "renew": true }));
                        }
                    }
                    in_background(
                        move || bodies.into_iter().map(|b| request(&s, "PUT", "/api/publication", Some(b))).collect::<Vec<_>>(),
                        move |results| {
                            button2.set_sensitive(true);
                            let mut mine = None;
                            for (p, r) in targets2.iter().zip(results) {
                                match r {
                                    Ok(v) => {
                                        let id = text(&v, "id");
                                        p.with_contest(|c| c.public_id = Some(id.clone()));
                                        p.mark_dirty();
                                        remember_pushed(p);
                                        if Rc::ptr_eq(p, &page2) {
                                            mine = Some(v);
                                        }
                                    }
                                    Err(e) => message2.set_text(&e),
                                }
                            }
                            if let Some(v) = mine {
                                message2.set_text("");
                                show2(&v);
                            }
                            let _ = &app2;
                        },
                    );
                },
            );
        });
    }
    {
        let (app, page, show, message) = (app.clone(), page.clone(), show_status.clone(), message.clone());
        unpublish.connect_clicked(move |_| {
            let Some(id) = page.with_contest(|c| c.public_id.clone()).flatten() else { return };
            if !dialogs::confirm(Some(&app.window), &tr("Unpublish this competition?"), Some(&tr("Its public page and QR code will stop working. You can publish it again (new address).")), None) {
                return;
            }
            let s = Settings::load(&app);
            let (page2, show2, message2) = (page.clone(), show.clone(), message.clone());
            in_background(move || request(&s, "DELETE", &format!("/api/publication/{id}"), None), move |r| match r {
                Ok(v) => {
                    page2.with_contest(|c| {
                        c.public_id = None;
                        c.public_event = None;
                    });
                    page2.mark_dirty();
                    show2(&v);
                }
                Err(e) => message2.set_text(&e),
            });
        });
    }
    dialog.content_area().append(&content);
    dialogs::run_dialog(&dialog);
    dialog.destroy();
}

// ------------------------------------------------- point de scission

/// Point de scission franchi : si la compétition est publiée, ses divisions
/// le sont aussi (mêmes options), dans son événement (créé s'il n'existe pas).
pub fn publish_split(app: &Rc<App>, source: &Rc<ContestPage>, divisions: Vec<Rc<ContestPage>>) {
    let s = Settings::load(app);
    if !s.configured() || divisions.is_empty() {
        return;
    }
    let Some((Some(pid), event, name, category, date, location)) = source.with_contest(|c| {
        (c.public_id.clone(), c.public_event.clone(), c.name.clone(), c.category.display_image(), format!("{:04}-{:02}-{:02}", c.year, c.month, c.day), c.location.clone())
    }) else {
        return;
    };
    let s2 = s.clone();
    let source = source.clone();
    in_background(
        move || -> Result<(String, String, bool), String> {
            let status = request(&s2, "GET", &format!("/api/publication/{pid}"), None)?;
            if !status.get("published").and_then(Value::as_bool).unwrap_or(false) {
                return Err("compétition non publiée".into());
            }
            let names = text(&status, "names");
            let noindex = status.get("noindex").and_then(Value::as_bool).unwrap_or(false);
            let event_id = match status.get("event").filter(|e| !e.is_null()).map(|e| text(e, "id")) {
                Some(id) => id,
                None => {
                    let body = json!({ "id": event, "name": name, "categories": category, "date": date, "location": location, "noindex": noindex });
                    text(&request(&s2, "PUT", "/api/publication/event", Some(body))?, "id")
                }
            };
            Ok((event_id, names, noindex))
        },
        move |r| {
            let Ok((event_id, names, noindex)) = r else { return };
            // La compétition rejoint l'événement (envoyée par la mise à jour automatique).
            source.with_contest(|c| c.public_event = Some(event_id.clone()));
            source.mark_dirty();
            let mut bodies = Vec::new();
            for p in &divisions {
                if let Some(xml) = p.with_contest(|c| {
                    c.public_event = Some(event_id.clone());
                    c.to_xml()
                }) {
                    bodies.push(json!({ "xml": xml, "names": names, "noindex": noindex, "renew": true }));
                }
            }
            in_background(
                move || bodies.into_iter().map(|b| request(&s, "PUT", "/api/publication", Some(b))).collect::<Vec<_>>(),
                move |results| {
                    for (p, r) in divisions.iter().zip(results) {
                        if let Ok(v) = r {
                            let id = text(&v, "id");
                            p.with_contest(|c| c.public_id = Some(id));
                            p.mark_dirty();
                            remember_pushed(p);
                        }
                    }
                },
            );
        },
    );
}

// ------------------------------------------------- mises à jour en direct

thread_local! {
    /// Empreinte du dernier envoi de chaque épreuve publiée.
    static PUSHED: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static PUSHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn fingerprint(xml: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    xml.hash(&mut h);
    h.finish()
}

fn remember_pushed(p: &Rc<ContestPage>) {
    if let Some((Some(id), xml)) = p.with_contest(|c| (c.public_id.clone(), c.to_xml())) {
        PUSHED.with(|m| m.borrow_mut().insert(id, fingerprint(&xml)));
    }
}

/// Envoie au serveur les épreuves publiées qui ont changé (appelé toutes les
/// 3 secondes par la liaison) : la page publique suit en direct.
pub fn push_updates(app: &Rc<App>, settings: &Settings) {
    if PUSHING.with(|p| p.get()) {
        return;
    }
    let pages: Vec<Rc<ContestPage>> = app.pages.borrow().clone();
    let mut bodies = Vec::new();
    for p in &pages {
        let Some(Some((id, xml))) = p.try_with_contest(|c| c.public_id.clone().map(|id| (id, c.to_xml()))) else { continue };
        let print = fingerprint(&xml);
        if PUSHED.with(|m| m.borrow().get(&id) == Some(&print)) {
            continue;
        }
        bodies.push((id, print, json!({ "xml": xml })));
    }
    if bodies.is_empty() {
        return;
    }
    PUSHING.with(|p| p.set(true));
    let s = settings.clone();
    let app = app.clone();
    in_background(
        move || {
            bodies
                .into_iter()
                .map(|(id, print, b)| {
                    let r = request(&s, "PUT", "/api/publication", Some(b));
                    let gone = matches!(&r, Err(e) if e.starts_with("HTTP 410"));
                    (id, print, r.is_ok(), gone)
                })
                .collect::<Vec<_>>()
        },
        move |results| {
            PUSHING.with(|p| p.set(false));
            for (id, print, ok, gone) in results {
                if ok {
                    PUSHED.with(|m| m.borrow_mut().insert(id, print));
                } else if gone {
                    // Dépubliée ailleurs (version web) : la compétition l'oublie.
                    for p in app.pages.borrow().iter() {
                        let forgot = p.with_contest(|c| {
                            if c.public_id.as_deref() == Some(id.as_str()) {
                                c.public_id = None;
                                c.public_event = None;
                                true
                            } else {
                                false
                            }
                        });
                        if forgot == Some(true) {
                            p.mark_dirty();
                        }
                    }
                }
            }
        },
    );
}
