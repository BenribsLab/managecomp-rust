//! Départage des ex æquo à la limite du quota, comme `barrage.cpp`, avec en
//! plus les matchs de barrage : feuilles de match et saisie des scores.

use bp_core::{filter::Filter, i18n::tr, rounds::barrage::Barrage, stage::Stage};
use bp_gtk::{dialogs, glade::Glade};
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use bp_core::player::PlayerRef;

use super::StageScreen;
use crate::contest_page::ContestPage;
use crate::widgets::{
    filter_dialog,
    list_document::{self, ListDocument},
    players_list::PlayersListView,
    render,
};

/// Un match de barrage : ses deux tireurs (référence, identité, score saisi).
#[derive(Clone)]
struct Bout {
    name: String,
    fencers: Vec<(PlayerRef, String, String)>,
    /// Identités imprimées (balisage) des deux tireurs.
    markups: Vec<String>,
    winner: Option<String>,
}

struct Snapshot {
    main: ListDocument,
    classification: ListDocument,
    locked: bool,
    qualified: u32,
    dropped: u32,
    bouts: Vec<Bout>,
    max_score: u32,
    places: u32,
    stage_name: String,
}

pub struct BarrageScreen {
    page: Weak<ContestPage>,
    index: usize,
    glade: Glade,
    root: gtk::Widget,
    list: PlayersListView,
    classification: PlayersListView,
    bouts_frame: gtk::Frame,
    bouts_grid: RefCell<gtk::Grid>,
    me: RefCell<Weak<BarrageScreen>>,
}

/// Identité imprimée d'un tireur : nom, prénom, club.
fn identity(players: &bp_core::player::PlayerStore, r: PlayerRef) -> String {
    let Some(p) = players.get(r) else { return String::new() };
    ["name", "first_name", "club"]
        .iter()
        .filter_map(|c| p.str_attr(c))
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join("  ")
}

impl BarrageScreen {
    pub fn new(page: &Rc<ContestPage>, index: usize) -> Rc<Self> {
        let glade = Glade::new("barrage.ui").expect("barrage.ui");
        let root: gtk::Widget = glade.get("root");
        Glade::detach_from_parent(&root);
        let list = PlayersListView::in_scrolled(glade.get("main_hook"));
        let classification = PlayersListView::new();
        glade
            .get::<gtk::Viewport>("classification_hook")
            .set_child(Some(classification.widget()));
        // Matchs de barrage, entre la barre d'outils et la liste.
        let bouts_frame = gtk::Frame::new(Some(&tr("Barrage bouts")));
        bouts_frame.set_margin_start(6);
        bouts_frame.set_margin_end(6);
        bouts_frame.set_margin_top(6);
        bouts_frame.set_margin_bottom(6);
        if let Some(root_box) = root.downcast_ref::<gtk::Box>() {
            root_box.insert_child_after(&bouts_frame, Some(&glade.get::<gtk::Widget>("toolbar3")));
        }
        let screen = Rc::new(Self {
            page: Rc::downgrade(page),
            index,
            glade,
            root,
            list,
            classification,
            bouts_frame,
            bouts_grid: RefCell::new(gtk::Grid::new()),
            me: RefCell::new(Weak::new()),
        });
        *screen.me.borrow_mut() = Rc::downgrade(&screen);
        macro_rules! on {
            ($handler:expr, $method:ident) => {{
                let weak = Rc::downgrade(&screen);
                screen.glade.connect_simple($handler, move |_| {
                    if let Some(s) = weak.upgrade() {
                        s.$method();
                    }
                });
            }};
        }
        on!(
            "on_barrage_classification_toggletoolbutton_toggled",
            toggle_classification
        );
        on!("on_barrage_filter_toolbutton_clicked", on_filter);
        on!("on_barrage_print_toolbutton_clicked", on_print);
        let weak = Rc::downgrade(&screen);
        screen.list.connect_toggled(move |targets, code, value| {
            let Some(s) = weak.upgrade() else { return };
            if code != "promoted" {
                return;
            }
            let Some(page) = s.page.upgrade() else { return };
            let changed = page
                .with_contest(|c| {
                    c.with_schedule(|schedule, ctx| {
                        let stage = schedule
                            .stage_mut(s.index)?
                            .as_any_mut()
                            .downcast_mut::<Barrage>()?;
                        if stage.base().locked {
                            return None;
                        }
                        for player in targets
                            .iter()
                            .filter(|p| stage.barrage_players().contains(p))
                            .copied()
                            .collect::<Vec<_>>()
                        {
                            stage.set_promoted(ctx, player, value);
                        }
                        Some(())
                    })
                })
                .flatten()
                .is_some();
            if changed {
                page.mark_dirty();
                s.refresh();
            }
            let schedule = page.schedule.borrow().clone();
            if let Some(schedule) = schedule {
                schedule.refresh_sensitivity();
            }
        });
        screen.refresh();
        screen
    }

    fn filter(classification: bool) -> Filter {
        let mut excluded = vec![
            "ref",
            "plugin_ID",
            "incident",
            "IP",
            "password",
            "cyphered_password",
            "HS",
            "attending",
            "exported",
            "final_rank",
            "global_status",
            "indice",
            "level",
            "workload_rate",
            "pool_nr",
            "team",
            "victories_count",
            "bouts_count",
            "victories_ratio",
            "strip",
            "time",
        ];
        if classification {
            excluded.push("promoted");
        } else {
            excluded.extend(["rank", "status"]);
        }
        Filter::excluding(
            if classification {
                "barrage_classification"
            } else {
                "barrage"
            },
            &excluded,
        )
    }

    fn snapshot(&self) -> Option<Snapshot> {
        self.page
            .upgrade()?
            .with_contest(|c| {
                let title = c.name.clone();
                c.with_schedule(|schedule, ctx| {
                    let stage = schedule
                        .stage(self.index)?
                        .as_any()
                        .downcast_ref::<Barrage>()?;
                    let base = stage.base();
                    let mut filter = Self::filter(false);
                    filter.apply_list(&base.filter);
                    let mut specs = render::column_specs(filter.layout(), !base.locked);
                    // Avec des matchs, le résultat désigne les promus.
                    list_document::make_editable(&mut specs, "promoted", !base.locked && stage.matches().is_empty());
                    let rows = list_document::rows_for(
                        ctx.players,
                        stage.barrage_players(),
                        &specs,
                        base.owner(),
                    );
                    let main = ListDocument {
                        title: title.clone(),
                        subtitle: tr("Barrage"),
                        specs,
                        rows,
                        classification_borders: false,
                    };
                    let mut filter = Self::filter(true);
                    filter.apply_list(&base.classification_filter);
                    let specs = render::column_specs(filter.layout(), false);
                    let rows = list_document::ranked_rows(
                        ctx.players,
                        &stage.classification(ctx),
                        &specs,
                        base.owner(),
                    );
                    let classification = ListDocument {
                        title,
                        subtitle: tr("Classification"),
                        specs,
                        rows,
                        classification_borders: true,
                    };
                    let bouts = stage
                        .matches()
                        .iter()
                        .map(|m| Bout {
                            name: m.name.clone(),
                            fencers: (0..2)
                                .filter_map(|i| {
                                    let f = m.opponent(i)?;
                                    Some((f, identity(ctx.players, f), m.score_at(i).image(m.max_score)))
                                })
                                .collect(),
                            markups: (0..2)
                                .filter_map(|i| m.opponent(i))
                                .map(|f| crate::widgets::score_sheet::fencer_markup(ctx.players, f))
                                .collect(),
                            winner: m.winner().map(|w| identity(ctx.players, w)),
                        })
                        .collect();
                    Some(Snapshot {
                        main,
                        classification,
                        locked: base.locked,
                        qualified: stage.qualified_count(ctx.players),
                        dropped: stage.dropped_count(ctx.players),
                        bouts,
                        max_score: stage.max_score(),
                        places: stage.places(),
                        stage_name: base.name.clone(),
                    })
                })
            })
            .flatten()
    }

    fn is_classification(&self) -> bool {
        self.glade
            .get::<gtk::ToggleButton>("barrage_classification_toggletoolbutton")
            .is_active()
    }
    fn toggle_classification(&self) {
        let classification = self.is_classification();
        self.glade
            .get::<gtk::Widget>("main_hook")
            .set_visible(!classification);
        self.glade
            .get::<gtk::Widget>("classification_hook")
            .set_visible(classification);
    }
    fn on_filter(&self) {
        let Some(page) = self.page.upgrade() else {
            return;
        };
        let classification = self.is_classification();
        let current = page
            .with_contest(|c| {
                let stage = c.schedule.stage(self.index)?;
                Some(if classification {
                    stage.base().classification_filter.clone()
                } else {
                    stage.base().filter.clone()
                })
            })
            .flatten();
        let Some(current) = current else { return };
        let mut filter = Self::filter(classification);
        filter.apply_list(&current);
        if filter_dialog::select_attributes(page.window().as_ref(), &mut filter, None) {
            page.with_contest(|c| {
                if let Some(stage) = c.schedule.stage_mut(self.index) {
                    if classification {
                        stage.base_mut().classification_filter = filter.selected_codes();
                    } else {
                        stage.base_mut().filter = filter.selected_codes();
                    }
                }
            });
            page.mark_dirty();
            self.refresh();
        }
    }
    /// Score d'un match de barrage ; passe à la case suivante si complet.
    fn on_score(self: &Rc<Self>, bout: usize, fencer: PlayerRef, value: String, next: Option<(usize, usize)>) {
        let Some(page) = self.page.upgrade() else { return };
        let complete = page
            .with_contest(|c| {
                c.with_schedule(|schedule, ctx| {
                    let stage = schedule.stage_mut(self.index)?.as_any_mut().downcast_mut::<Barrage>()?;
                    Some(stage.set_score(ctx, bout, fencer, &value))
                })
            })
            .flatten()
            .unwrap_or(false);
        page.mark_dirty();
        self.refresh();
        if let Some(schedule) = page.schedule.borrow().clone() {
            schedule.refresh_sensitivity();
        }
        if let (true, Some((row, col))) = (complete, next) {
            let grid = self.bouts_grid.borrow().clone();
            if let Some(entry) = grid.child_at(col as i32, row as i32).and_then(|w| w.downcast::<gtk::Entry>().ok()) {
                crate::widgets::focus_entry_when_ready(&entry, true, -1);
            }
        }
    }

    /// Grille des matchs : nom, tireur A, score, score, tireur B, vainqueur.
    fn fill_bouts(self: &Rc<Self>, s: &Snapshot) {
        self.bouts_frame.set_visible(!s.bouts.is_empty());
        if s.bouts.is_empty() {
            return;
        }
        let grid = gtk::Grid::new();
        grid.set_row_spacing(4);
        grid.set_column_spacing(8);
        grid.set_margin_start(8);
        grid.set_margin_end(8);
        grid.set_margin_top(6);
        grid.set_margin_bottom(6);
        let info = gtk::Label::new(Some(&format!(
            "{} {} — {} : {}",
            tr("Bouts to"),
            s.max_score,
            tr("Places to award"),
            s.places
        )));
        info.set_xalign(0.0);
        grid.attach(&info, 0, 0, 5, 1);
        let print = gtk::Button::with_label(&tr("Print score sheets"));
        let weak = Rc::downgrade(self);
        print.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.print_sheets();
            }
        });
        grid.attach(&print, 5, 0, 1, 1);
        // Ordre de saisie : chaque match, tireur du haut puis du bas.
        let cells: Vec<(usize, usize)> = (0..s.bouts.len()).flat_map(|i| [(i + 1, 2), (i + 1, 3)]).collect();
        for (i, bout) in s.bouts.iter().enumerate() {
            let row = i as i32 + 1;
            let name = gtk::Label::new(None);
            name.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(&bout.name)));
            name.set_xalign(0.0);
            grid.attach(&name, 0, row, 1, 1);
            for (k, (fencer, identity, score)) in bout.fencers.iter().enumerate() {
                let label = gtk::Label::new(Some(identity));
                label.set_xalign(if k == 0 { 1.0 } else { 0.0 });
                grid.attach(&label, if k == 0 { 1 } else { 4 }, row, 1, 1);
                let entry = gtk::Entry::new();
                entry.set_width_chars(4);
                entry.set_max_width_chars(4);
                entry.set_text(score);
                entry.set_sensitive(!s.locked);
                let col = 2 + k;
                let next = cells.iter().position(|c| *c == (i + 1, col)).and_then(|p| cells.get(p + 1)).copied();
                let fencer = *fencer;
                let weak = Rc::downgrade(self);
                entry.connect_activate(move |e| {
                    if let Some(s) = weak.upgrade() {
                        s.on_score(i, fencer, e.text().to_string(), next);
                    }
                });
                let weak = Rc::downgrade(self);
                let max = s.max_score;
                entry.connect_changed(move |e| {
                    let text = e.text().to_string();
                    if crate::widgets::entry_has_focus(e) && score_complete(&text, max) {
                        if let Some(s) = weak.upgrade() {
                            let e = e.clone();
                            gtk::glib::idle_add_local_once(move || s.on_score(i, fencer, e.text().to_string(), next));
                        }
                    }
                });
                grid.attach(&entry, col as i32, row, 1, 1);
            }
            let winner = gtk::Label::new(None);
            if let Some(w) = &bout.winner {
                winner.set_markup(&format!("→ <b>{}</b>", gtk::glib::markup_escape_text(w)));
            }
            winner.set_xalign(0.0);
            grid.attach(&winner, 5, row, 1, 1);
        }
        self.bouts_frame.set_child(Some(&grid));
        *self.bouts_grid.borrow_mut() = grid;
    }

    /// Feuilles des matchs de barrage (quatre par page).
    fn print_sheets(&self) {
        let Some(s) = self.snapshot() else { return };
        let parent = self.page.upgrade().and_then(|p| p.window());
        let sheets = Rc::new(SheetsSource { stage: s.stage_name.clone(), bouts: s.bouts.clone(), max_score: s.max_score });
        let job = format!("{} — {}", s.main.title, tr("Score sheets"));
        let result = bp_gtk::print::run_document(parent.as_ref(), &job, move || Box::new(SheetsSource::clone(&sheets)));
        if let Err(e) = result {
            dialogs::error(parent.as_ref(), &e.to_string());
        }
    }

    fn on_print(&self) {
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        let parent = self.page.upgrade().and_then(|p| p.window());
        let document = if self.is_classification() {
            snapshot.classification
        } else {
            snapshot.main
        };
        if let Err(e) = document.print(parent.as_ref()) {
            dialogs::error(parent.as_ref(), &e.to_string());
        }
    }
}

/// Saisie complète : `V`, `W3`, ou un nombre aussi long que le score maximum.
fn score_complete(value: &str, max: u32) -> bool {
    let v = value.trim().to_ascii_uppercase();
    if v == "V" {
        return true;
    }
    let digits = v.trim_start_matches(['V', 'W']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    digits.len() >= max.max(1).to_string().len() || format!("{digits}0").parse::<u32>().map(|n| n > max).unwrap_or(true)
}

#[derive(Clone)]
struct SheetsSource {
    stage: String,
    bouts: Vec<Bout>,
    max_score: u32,
}

impl SheetsSource {
    fn sheet(&self, bout: &Bout) -> crate::widgets::score_sheet::SheetInfo {
        crate::widgets::score_sheet::SheetInfo {
            name: if self.stage.is_empty() { bout.name.clone() } else { format!("{} — {}", self.stage, bout.name) },
            referee: None,
            officiating: String::new(),
            piste: None,
            fencers: bout.markups.clone(),
            max_score: self.max_score,
            flash_ref: None,
            qr: None,
        }
    }
}

impl bp_gtk::print::PageSource for SheetsSource {
    fn page_name(&self, _page: usize) -> String {
        format!("{} - {}", self.stage, tr("Score sheets"))
    }

    fn begin(&mut self, width: f64, height: f64) -> usize {
        let per_page = if width > height { 2 } else { 4 };
        self.bouts.len().div_ceil(per_page).max(1)
    }

    fn draw(&mut self, page: usize, cr: &cairo::Context, width: f64, height: f64) {
        let per_page = if width > height { 2 } else { 4 };
        let sheets: Vec<_> = self.bouts.iter().skip(page * per_page).take(per_page).map(|b| self.sheet(b)).collect();
        crate::widgets::score_sheet::draw_page(cr, width, height, &sheets, per_page);
    }
}

impl StageScreen for BarrageScreen {
    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }
    fn refresh(&self) {
        let Some(s) = self.snapshot() else { return };
        if let Some(me) = self.me.borrow().upgrade() {
            me.fill_bouts(&s);
        }
        self.list.set_columns(s.main.specs);
        self.list.set_rows(s.main.rows);
        self.list.set_locked(s.locked);
        self.classification.set_columns(s.classification.specs);
        self.classification.set_rows(s.classification.rows);
        self.glade
            .get::<gtk::Label>("promoted_label")
            .set_text(&s.qualified.to_string());
        self.glade
            .get::<gtk::Label>("dropped_label")
            .set_text(&s.dropped.to_string());
        self.toggle_classification();
    }
}
