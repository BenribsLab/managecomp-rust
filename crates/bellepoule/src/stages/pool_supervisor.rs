//! Feuille de poule, saisie des scores et classements courant / cumulé.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use bp_core::contest::Contest;
use bp_core::filter::Filter;
use bp_core::i18n::tr;
use bp_core::player::{Owner, PlayerRef, PlayerStore};
use bp_core::rounds::pool::{
    pool::{PoolSheet, PoolStatus},
    round::{FilterKind, PoolRound},
    PoolSupervisorStage,
};
use bp_core::stage::Stage;
use bp_gtk::{dialogs, glade::Glade};
use gtk::{glib, prelude::*};
use gtk4 as gtk;

use super::StageScreen;
use crate::contest_page::ContestPage;
use crate::widgets::{
    filter_dialog,
    players_list::{ColumnSpec, PlayersListView, RowData},
    render,
};

pub(crate) mod print;

// Match, tireur, vue (grille ou liste des matchs).
type Key = (usize, PlayerRef, bool);
struct Focus {
    pool: usize,
    key: Key,
    text: String,
    cursor: i32,
    advance: bool,
}

pub struct SupervisorScreen {
    page: Weak<ContestPage>,
    weak_self: Weak<Self>,
    index: usize,
    glade: Glade,
    root: gtk::Widget,
    configuration: gtk::Widget,
    sheet: gtk::ScrolledWindow,
    combined: PlayersListView,
    current: PlayersListView,
    selected: Cell<usize>,
    updating: Cell<bool>,
    entries: RefCell<HashMap<Key, gtk::Entry>>,
    order: RefCell<Vec<(usize, PlayerRef)>>,
    /// Ordre de saisie dans la grille : ligne par ligne, de gauche à droite.
    grid_order: RefCell<Vec<(usize, PlayerRef)>>,
    refresh_pending: Cell<bool>,
    focus: RefCell<Option<Focus>>,
}

impl SupervisorScreen {
    pub fn new(page: &Rc<ContestPage>, index: usize) -> Rc<Self> {
        crate::widgets::install_sheet_css();
        let glade = Glade::new("pool_supervisor.ui").expect("pool_supervisor.ui");
        let root: gtk::Widget = glade.get("root");
        let configuration: gtk::Widget = glade.get("stage_configuration");
        Glade::detach_from_parent(&root);
        Glade::detach_from_parent(&configuration);
        let sheet = gtk::ScrolledWindow::new();
        sheet.set_hexpand(true);
        sheet.set_vexpand(true);
        glade
            .get::<gtk::Viewport>("main_hook")
            .set_child(Some(&sheet));
        let combined = PlayersListView::new();
        let current = PlayersListView::new();
        glade
            .get::<gtk::Viewport>("coumpound_classification_hook")
            .set_child(Some(combined.widget()));
        glade
            .get::<gtk::Viewport>("current_round_classification_hook")
            .set_child(Some(current.widget()));
        let screen = Rc::new_cyclic(|weak_self| Self {
            page: Rc::downgrade(page),
            weak_self: weak_self.clone(),
            index,
            glade,
            root,
            configuration,
            sheet,
            combined,
            current,
            selected: Cell::new(0),
            updating: Cell::new(false),
            entries: RefCell::new(HashMap::new()),
            order: RefCell::new(Vec::new()),
            grid_order: RefCell::new(Vec::new()),
            refresh_pending: Cell::new(false),
            focus: RefCell::new(None),
        });
        screen.connect();
        screen.refresh();
        screen
    }

    fn connect(self: &Rc<Self>) {
        macro_rules! on {
            ($name:expr, $method:ident) => {{
                let weak = self.weak_self.clone();
                self.glade.connect_simple($name, move |_| {
                    if let Some(screen) = weak.upgrade() {
                        screen.$method();
                    }
                });
            }};
        }
        on!("on_pool_combobox_changed", on_selected);
        on!(
            "on_pool_classification_toggletoolbutton_toggled",
            toggle_classification
        );
        on!("on_pool_filter_toolbutton_clicked", on_filter);
        on!("on_print_pool_toolbutton_clicked", on_print);
        on!("on_stuff_toolbutton_clicked", on_stuff);
        on!("on_all_radiobutton_toggled", on_config);
        on!("on_for_referees_radiobutton_clicked", update_print_options);
        for id in ["name_entry", "max_score_entry"] {
            let weak = self.weak_self.clone();
            self.glade.get::<gtk::Entry>(id).connect_changed(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.on_config();
                }
            });
        }
        let weak = self.weak_self.clone();
        self.glade
            .get::<gtk::SpinButton>("nb_qualified_spinbutton")
            .connect_value_changed(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.on_config();
                }
            });
        let weak = self.weak_self.clone();
        self.glade
            .get::<gtk::CheckButton>("balanced_radiobutton")
            .connect_toggled(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.on_config();
                }
            });
    }

    fn edit<R>(&self, f: impl FnOnce(&mut PoolRound, &mut PlayerStore) -> R) -> Option<R> {
        let page = self.page.upgrade()?;
        let result = page
            .with_contest(|c| {
                c.with_schedule(|schedule, ctx| {
                    let stage = schedule
                        .stage(self.index)?
                        .as_any()
                        .downcast_ref::<PoolSupervisorStage>()?;
                    if stage.base().locked {
                        return None;
                    }
                    let result = f(&mut stage.round(), ctx.players);
                    Some(result)
                })
            })
            .flatten();
        if result.is_some() {
            page.mark_dirty();
        }
        result
    }

    fn queue_refresh(&self) {
        if self.refresh_pending.replace(true) {
            return;
        }
        let weak = self.weak_self.clone();
        glib::idle_add_local_once(move || {
            let Some(s) = weak.upgrade() else { return };
            s.refresh_pending.set(false);
            let mut focus = s.focus.borrow_mut().take();
            if let Some(focus) = focus.as_mut() {
                if let Some(entry) = s.entries.borrow().get(&focus.key) {
                    // GtkEditable termine la mise à jour du curseur après changed.
                    focus.cursor = entry.position();
                }
            }
            s.refresh();
            if let Some(focus) = focus.filter(|f| f.pool == s.selected.get()) {
                let key = if focus.advance {
                    // Grille : case suivante de la ligne ; liste des matchs : match suivant.
                    let order = if focus.key.2 { s.order.borrow() } else { s.grid_order.borrow() };
                    order
                        .iter()
                        .position(|k| *k == (focus.key.0, focus.key.1))
                        .and_then(|i| order.get(i + 1))
                        .map(|(m, p)| (*m, *p, focus.key.2))
                        .unwrap_or(focus.key)
                } else {
                    focus.key
                };
                if let Some(entry) = s.entries.borrow().get(&key) {
                    s.updating.set(true);
                    if !focus.advance {
                        entry.set_text(&focus.text);
                    }
                    crate::widgets::focus_entry_when_ready(entry, focus.advance, focus.cursor);
                    s.updating.set(false);
                }
            }
            if let Some(page) = s.page.upgrade() {
                if let Some(schedule) = page.schedule.borrow().as_ref() {
                    schedule.refresh_sensitivity();
                }
            }
        });
    }

    fn score_entry(
        &self,
        match_index: usize,
        player: PlayerRef,
        text: &str,
        list: bool,
        locked: bool,
        error: bool,
    ) -> gtk::Entry {
        let entry = gtk::Entry::new();
        entry.set_width_chars(3);
        entry.set_max_width_chars(4);
        entry.set_max_length(6);
        gtk::prelude::EntryExt::set_alignment(&entry, 0.5);
        entry.set_text(text);
        entry.set_editable(!locked);
        entry.set_can_focus(!locked);
        entry.set_tooltip_text(Some("V · W3 / V3 · 0–999"));
        if error {
            entry.add_css_class("error");
        }
        let key = (match_index, player, list);
        self.entries.borrow_mut().insert(key, entry.clone());
        let weak = self.weak_self.clone();
        entry.connect_changed(move |entry| {
            let Some(s) = weak.upgrade() else { return };
            if s.updating.get() {
                return;
            }
            let text = entry.text().to_string();
            let upper = text.to_ascii_uppercase();
            let digits = upper.strip_prefix(['V', 'W']).unwrap_or(&upper);
            if !digits.bytes().all(|b| b.is_ascii_digit()) || !text.is_ascii() {
                entry.add_css_class("error");
                return;
            }
            let input = if upper.starts_with('V') && upper.len() > 1 {
                format!("W{}", &upper[1..])
            } else {
                upper
            };
            let pool = s.selected.get();
            let complete = s
                .edit(|round, players| {
                    let complete = round
                        .pools_mut()
                        .get_mut(pool)
                        .map(|p| p.set_score_str(players, match_index, player, &input))
                        .unwrap_or(false);
                    round.refresh_classification(players);
                    complete
                })
                .unwrap_or(false);
            if crate::widgets::entry_has_focus(entry) {
                *s.focus.borrow_mut() = Some(Focus {
                    pool,
                    key,
                    text,
                    cursor: entry.position(),
                    advance: complete,
                });
            }
            s.queue_refresh();
        });
        let weak = self.weak_self.clone();
        entry.connect_activate(move |entry| {
            if let Some(s) = weak.upgrade() {
                *s.focus.borrow_mut() = Some(Focus {
                    pool: s.selected.get(),
                    key,
                    text: entry.text().to_string(),
                    cursor: entry.position(),
                    advance: true,
                });
                s.queue_refresh();
            }
        });
        entry
    }

    /// Cellule de la feuille avec son contour ; `head` : en-tête de colonne.
    fn bordered(grid: &gtk::Grid, text: &str, col: usize, row: usize, head: bool) {
        let label = gtk::Label::new(Some(text));
        label.set_xalign(if head { 0.5 } else { 0.0 });
        label.set_hexpand(false);
        label.add_css_class("bp-cell");
        if head {
            label.add_css_class("bp-head");
        }
        grid.attach(&label, col as i32, row as i32, 1, 1);
    }

    fn label(grid: &gtk::Grid, text: &str, col: usize, row: usize) {
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.set_margin_start(4);
        label.set_margin_end(4);
        grid.attach(&label, col as i32, row as i32, 1, 1);
    }

    fn display_sheet(
        &self,
        sheet: &PoolSheet,
        identities: &[Vec<String>],
        locked: bool,
        errors: &[usize],
    ) {
        self.entries.borrow_mut().clear();
        self.order.borrow_mut().clear();
        self.grid_order.borrow_mut().clear();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_top(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_bottom(12);
        let title = gtk::Label::new(Some(&sheet.title));
        title.add_css_class("title-2");
        title.set_xalign(0.0);
        content.append(&title);
        let details = format!(
            "{}   {}",
            sheet.piste.as_deref().unwrap_or(""),
            sheet
                .referee
                .as_ref()
                .map(|(n, f)| format!("{} {}", n, f.as_deref().unwrap_or("")))
                .unwrap_or_default()
        );
        let roadmap = gtk::Label::new(Some(&details));
        roadmap.set_xalign(0.0);
        content.append(&roadmap);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(0);
        grid.set_column_spacing(0);
        grid.add_css_class("bp-sheet");
        let identity_columns = identities.iter().map(Vec::len).max().unwrap_or(1);
        let score_start = identity_columns + 1;
        let dashboard_start = score_start + sheet.rows.len();
        for i in 0..sheet.rows.len() {
            Self::bordered(&grid, &(i + 1).to_string(), score_start + i, 0, true);
        }
        for (i, heading) in sheet.headers.iter().enumerate() {
            Self::bordered(&grid, heading, dashboard_start + i, 0, true);
        }
        for (i, row) in sheet.rows.iter().enumerate() {
            Self::bordered(&grid, &row.position.to_string(), 0, i + 1, false);
            if let Some(identity) = identities.get(i) {
                for (j, text) in identity.iter().enumerate() {
                    Self::bordered(&grid, text, j + 1, i + 1, false);
                }
            }
            for (j, cell) in row.cells.iter().enumerate() {
                if let Some(mi) = cell.match_index.filter(|_| !cell.grey) {
                    self.grid_order.borrow_mut().push((mi, row.player));
                    let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    frame.add_css_class("bp-cell");
                    frame.append(&self.score_entry(
                        mi,
                        row.player,
                        &cell.text,
                        false,
                        locked,
                        errors.contains(&mi),
                    ));
                    grid.attach(&frame, (score_start + j) as i32, (i + 1) as i32, 1, 1);
                } else {
                    let blank = gtk::Label::new(Some("     "));
                    blank.add_css_class("bp-cell");
                    blank.add_css_class("bp-grey");
                    grid.attach(&blank, (score_start + j) as i32, (i + 1) as i32, 1, 1);
                }
            }
            if let Some(d) = &row.dashboard {
                let status = gtk::ComboBoxText::new();
                for (code, text) in [
                    ("Q", "Qualified"),
                    ("A", "Withdrawal"),
                    ("E", "Excluded"),
                    ("F", "Forfeit"),
                ] {
                    status.append(Some(code), &tr(text));
                }
                status.set_active_id(Some(&d.status.to_string()));
                bp_gtk::util::scroll_passes_through(&status);
                status.set_sensitive(!locked);
                let weak = self.weak_self.clone();
                let player = row.player;
                let pool = self.selected.get();
                status.connect_changed(move |combo| {
                    if let (Some(s), Some(code)) = (weak.upgrade(), combo.active_id()) {
                        if s.updating.get() {
                            return;
                        }
                        s.edit(|round, players| {
                            if let Some(p) = round.pools_mut().get_mut(pool) {
                                p.set_status(players, player, &code);
                            }
                            round.refresh_classification(players);
                        });
                        s.queue_refresh();
                    }
                });
                let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                frame.add_css_class("bp-cell");
                frame.append(&status);
                grid.attach(&frame, dashboard_start as i32, (i + 1) as i32, 1, 1);
                for (j, value) in [
                    &d.victories,
                    &d.victories_ratio,
                    &d.hits_scored,
                    &d.hits_received,
                    &d.indice,
                    &d.rank,
                ]
                .iter()
                .enumerate()
                {
                    Self::bordered(&grid, value, dashboard_start + j + 1, i + 1, false);
                }
            }
        }
        content.append(&grid);
        let bouts = gtk::Grid::new();
        bouts.set_row_spacing(4);
        bouts.set_column_spacing(8);
        for (i, bout) in sheet.matches.iter().enumerate() {
            let match_index = bout.match_index;
            Self::label(
                &bouts,
                &format!("{}{}", bout.name, if bout.rest_error { " ⏱" } else { "" }),
                0,
                i,
            );
            for (j, opponent) in bout.opponents.iter().enumerate() {
                if let Some(opponent) = opponent {
                    Self::label(
                        &bouts,
                        &format!(
                            "{}  {} {}",
                            opponent.position, opponent.identity.name, opponent.identity.first_name
                        ),
                        1 + j * 2,
                        i,
                    );
                    // Les matchs et la feuille partagent les mêmes indices du modèle.
                    let key = (match_index, opponent.player);
                    let editable = sheet
                        .rows
                        .iter()
                        .find(|r| r.player == opponent.player)
                        .is_some_and(|r| {
                            r.cells.iter().any(|c| c.match_index == Some(match_index) && !c.grey)
                        });
                    if editable {
                        self.order.borrow_mut().push(key);
                        bouts.attach(
                            &self.score_entry(
                                match_index,
                                opponent.player,
                                &opponent.score,
                                true,
                                locked,
                                errors.contains(&match_index),
                            ),
                            (2 + j * 2) as i32,
                            i as i32,
                            1,
                            1,
                        );
                    }
                }
            }
            if let Some(resource) = &bout.resource {
                Self::label(&bouts, resource, 5, i);
            }
        }
        content.append(&bouts);
        self.sheet.set_child(Some(&content));
    }

    fn rows(c: &Contest, refs: &[PlayerRef], specs: &[ColumnSpec], owner: Owner) -> Vec<RowData> {
        refs.iter()
            .filter_map(|r| {
                let p = c.players.get(*r)?;
                Some(RowData {
                    player: *r,
                    cells: specs
                        .iter()
                        .map(|s| render::cell_for(p, s, owner))
                        .collect(),
                    sort: render::sort_values(p, specs, owner),
                    children: Vec::new(),
                    dimmed: false,
                })
            })
            .collect()
    }

    fn on_selected(&self) {
        if self.updating.get() {
            return;
        }
        if let Some(index) = self.glade.get::<gtk::ComboBox>("pool_combobox").active() {
            self.selected.set(index as usize);
            self.focus.borrow_mut().take();
            self.refresh();
        }
    }

    fn toggle_classification(&self) {
        let active = self
            .glade
            .get::<gtk::ToggleButton>("pool_classification_toggletoolbutton")
            .is_active();
        self.glade
            .get::<gtk::Widget>("main_hook")
            .set_visible(!active);
        self.glade
            .get::<gtk::Widget>("classification_hook")
            .set_visible(active);
    }

    fn on_config(&self) {
        if self.updating.get() {
            return;
        }
        let name = self
            .glade
            .get::<gtk::Entry>("name_entry")
            .text()
            .to_string();
        let Ok(max_score) = self
            .glade
            .get::<gtk::Entry>("max_score_entry")
            .text()
            .parse::<u32>()
        else {
            return;
        };
        if max_score == 0 || max_score > 999 {
            return;
        }
        let all = self
            .glade
            .get::<gtk::CheckButton>("all_radiobutton")
            .is_active();
        let number = self
            .glade
            .get::<gtk::SpinButton>("nb_qualified_spinbutton")
            .value_as_int()
            .max(0) as u32;
        let balanced = self
            .glade
            .get::<gtk::CheckButton>("balanced_radiobutton")
            .is_active();
        self.edit(|round, players| {
            round.name = name;
            round.set_max_score(max_score);
            round.set_nb_qualified(if all { None } else { Some(number) });
            if round.seeding_is_balanced() != balanced {
                round.set_seeding_balanced(players, balanced);
            }
            for pool in round.pools_mut() {
                pool.refresh_score_data(players);
            }
            round.refresh_classification(players);
        });
        self.queue_refresh();
    }

    fn on_stuff(&self) {
        let allowed = self
            .page
            .upgrade()
            .and_then(|p| p.with_contest(|c| c.settings.score_stuffing_allowed))
            .unwrap_or(false);
        if allowed {
            self.edit(|round, players| round.stuff(players));
            self.queue_refresh();
        }
    }

    fn on_filter(&self) {
        let Some(page) = self.page.upgrade() else {
            return;
        };
        let classification = self
            .glade
            .get::<gtk::ToggleButton>("pool_classification_toggletoolbutton")
            .is_active();
        let kind = if classification {
            FilterKind::Classification
        } else {
            FilterKind::Supervisor
        };
        let current = page
            .with_contest(|c| {
                let stage = c
                    .schedule
                    .stage(self.index)?
                    .as_any()
                    .downcast_ref::<PoolSupervisorStage>()?;
                let round = stage.round();
                Some(if classification {
                    round.classification_filter.clone()
                } else {
                    round.supervisor_filter.clone()
                })
            })
            .flatten();
        let Some(current) = current else { return };
        let mut filter = Filter::including("pool_supervisor", &kind.available());
        filter.apply_list(&current);
        if filter_dialog::select_attributes(page.window().as_ref(), &mut filter, None) {
            page.with_contest(|c| {
                if let Some(stage) = c
                    .schedule
                    .stage(self.index)
                    .and_then(|s| s.as_any().downcast_ref::<PoolSupervisorStage>())
                {
                    let mut round = stage.round();
                    if classification {
                        round.classification_filter = filter.selected_codes();
                    } else {
                        round.supervisor_filter = filter.selected_codes();
                    }
                }
            });
            page.mark_dirty();
            self.refresh();
        }
    }

    fn on_print(&self) {
        let Some(page) = self.page.upgrade() else {
            return;
        };
        let classification = self
            .glade
            .get::<gtk::ToggleButton>("pool_classification_toggletoolbutton")
            .is_active();
        if classification {
            let single = self
                .glade
                .get::<gtk::Notebook>("classification_hook")
                .current_page()
                == Some(1);
            let data = page.with_contest(|c| classification_data(c, self.index, single)).flatten();
            if let Some((title, headers, rows)) = data {
                if let Err(e) = print::classification(page.window().as_ref(), &title, headers, rows)
                {
                    dialogs::error(page.window().as_ref(), &e.to_string());
                }
            }
            return;
        }
        // Rencontres par équipes : feuilles de match en relais.
        if super::relay_print::is_team(&page) {
            super::relay_print::print(&page, self.index, None, false);
            return;
        }
        // Entraînement des jeunes : on imprime toujours le groupe complet.
        if super::group_print::is_youth(&page, self.index) {
            super::group_print::print(&page, self.index, Some(self.selected.get()));
            return;
        }
        let dialog: gtk::MessageDialog = self.glade.get("print_pool_dialog");
        self.update_print_options();
        dialog.set_transient_for(page.window().as_ref());
        let response = dialogs::run_dialog(&dialog);
        dialog.hide();
        if response != gtk::ResponseType::Ok {
            return;
        }
        let all = self
            .glade
            .get::<gtk::CheckButton>("all_pool_radiobutton")
            .is_active();
        let referees = self
            .glade
            .get::<gtk::CheckButton>("for_referees_radiobutton")
            .is_active();
        let watermark = self
            .glade
            .get::<gtk::CheckButton>("watermark_checkbutton")
            .is_active();
        let selected = self.selected.get();
        let data = page
            .with_contest(|c| {
                let mut data = sheets_data(c, self.index, |i| all || i == selected, referees, true)?;
                if referees || !all {
                    data.density = (1, 1);
                }
                Some(data)
            })
            .flatten();
        if let Some(data) = data {
            if let Err(e) = print::sheets(page.window().as_ref(), data, watermark) {
                dialogs::error(page.window().as_ref(), &e.to_string());
            }
        }
    }

    fn update_print_options(&self) {
        let for_referees = self
            .glade
            .get::<gtk::CheckButton>("for_referees_radiobutton")
            .is_active();
        self.glade
            .get::<gtk::Widget>("watermark_checkbutton")
            .set_sensitive(for_referees);
    }
}

impl StageScreen for SupervisorScreen {
    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }
    fn configuration(&self) -> Option<gtk::Widget> {
        Some(self.configuration.clone())
    }

    fn refresh(&self) {
        let Some(page) = self.page.upgrade() else {
            return;
        };
        let data = page
            .with_contest(|c| {
                let stage = c
                    .schedule
                    .stage(self.index)?
                    .as_any()
                    .downcast_ref::<PoolSupervisorStage>()?;
                let mut round = stage.round();
                let locked = stage.base().locked;
                if !locked && round.has_matches() {
                    round.refresh_classification(&mut c.players);
                }
                let statuses: Vec<_> = round
                    .pools()
                    .iter()
                    .map(|p| (p.name().to_string(), p.status(c.referees.store())))
                    .collect();
                let selected = self.selected.get().min(statuses.len().saturating_sub(1));
                let sheet = round
                    .pool(selected)
                    .map(|p| p.sheet(&c.players, c.referees.store(), false));
                let errors: Vec<_> = round
                    .pool(selected)
                    .map(|p| {
                        p.matches()
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| m.bout.has_error())
                            .map(|(i, _)| i)
                            .collect()
                    })
                    .unwrap_or_default();
                let mut filter = Filter::including(
                    "pool_classification",
                    &FilterKind::Classification.available(),
                );
                filter.apply_list(&round.classification_filter);
                let specs = render::column_specs(filter.layout(), false);
                let (current, combined) = round.compute_classification(&c.players);
                let current = Self::rows(
                    c,
                    &current.into_iter().map(|(p, _)| p).collect::<Vec<_>>(),
                    &specs,
                    round.current_round_owner(),
                );
                let combined = Self::rows(
                    c,
                    &combined.into_iter().map(|(p, _)| p).collect::<Vec<_>>(),
                    &specs,
                    round.supervisor_owner(),
                );
                let identities = sheet
                    .as_ref()
                    .map(|sheet| {
                        sheet
                            .rows
                            .iter()
                            .map(|row| {
                                round
                                    .supervisor_filter
                                    .iter()
                                    .map(|code| {
                                        c.players
                                            .get(row.player)
                                            .and_then(|p| {
                                                render::value_of(p, code, round.allocator_owner())
                                            })
                                            .and_then(|v| {
                                                bp_core::attribute::registry().by_code(code).map(
                                                    |d| render::user_image(d, v, d.favorite_look),
                                                )
                                            })
                                            .unwrap_or_default()
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Some((
                    locked,
                    statuses,
                    selected,
                    sheet,
                    errors,
                    specs,
                    current,
                    combined,
                    identities,
                    round.name.clone(),
                    round.max_score,
                    round.nb_qualified(),
                    round.seeding_is_balanced(),
                    c.settings.score_stuffing_allowed,
                    c.schedule
                        .stage(self.index.saturating_sub(1))
                        .is_some_and(|s| s.base().locked),
                ))
            })
            .flatten();
        let Some((
            locked,
            statuses,
            selected,
            sheet,
            errors,
            specs,
            current,
            combined,
            identities,
            name,
            max,
            qualified,
            balanced,
            stuffing,
            allocator_locked,
        )) = data
        else {
            return;
        };
        self.updating.set(true);
        self.selected.set(selected);
        let store: gtk::ListStore = self.glade.get("pool_liststore");
        store.clear();
        for (name, status) in statuses {
            let icon_name = match status {
                PoolStatus::Over => "object-select-symbolic",
                PoolStatus::Error => "dialog-warning-symbolic",
                PoolStatus::RefereeConnection(_) => "network-error-symbolic",
                PoolStatus::Ongoing => "media-playback-start-symbolic",
            };
            let icon = bp_gtk::icons::icon_pixbuf(icon_name, 16);
            let iter = store.append();
            store.set(&iter, &[(0, &name), (1, &icon)]);
        }
        self.glade.get::<gtk::ComboBox>("pool_combobox").set_active(
            if store.iter_n_children(None) > 0 {
                Some(selected as u32)
            } else {
                None
            },
        );
        self.combined.set_columns(specs.clone());
        self.current.set_columns(specs);
        self.combined.set_rows(combined);
        self.current.set_rows(current);
        self.glade.get::<gtk::Entry>("name_entry").set_text(&name);
        self.glade
            .get::<gtk::Entry>("max_score_entry")
            .set_text(&max.to_string());
        self.glade
            .get::<gtk::SpinButton>("nb_qualified_spinbutton")
            .set_value(qualified.unwrap_or(0) as f64);
        self.glade
            .get::<gtk::Widget>("nb_qualified_spinbutton")
            .set_visible(qualified.is_some());
        self.glade
            .get::<gtk::CheckButton>("all_radiobutton")
            .set_active(qualified.is_none());
        self.glade
            .get::<gtk::CheckButton>("number_radiobutton")
            .set_active(qualified.is_some());
        self.glade
            .get::<gtk::CheckButton>("balanced_radiobutton")
            .set_active(balanced);
        self.configuration.set_sensitive(!locked);
        self.glade
            .get::<gtk::Widget>("seeding_viewport")
            .set_sensitive(!allocator_locked);
        self.glade
            .get::<gtk::Widget>("stuff_toolbutton")
            .set_visible(stuffing);
        self.glade
            .get::<gtk::Widget>("stuff_toolbutton")
            .set_sensitive(!locked);
        if let Some(sheet) = sheet {
            self.display_sheet(&sheet, &identities, locked, &errors);
        } else {
            self.sheet.set_child(gtk::Widget::NONE);
        }
        self.toggle_classification();
        self.updating.set(false);
    }
}

/// Classement du tour de poules (`single` : tour courant seul, sinon cumulé).
fn classification_data(c: &Contest, index: usize, single: bool) -> Option<(String, Vec<String>, Vec<Vec<String>>)> {
    let stage = c.schedule.stage(index)?.as_any().downcast_ref::<PoolSupervisorStage>()?;
    let round = stage.round();
    let (current, combined) = round.compute_classification(&c.players);
    let refs: Vec<_> = if single { current } else { combined }.into_iter().map(|(p, _)| p).collect();
    let owner = if single { round.current_round_owner() } else { round.supervisor_owner() };
    let columns: Vec<_> =
        round.classification_filter.iter().filter_map(|code| bp_core::attribute::registry().by_code(code)).collect();
    let headers = columns.iter().map(|d| tr(d.user_name)).collect();
    let rows = refs
        .iter()
        .filter_map(|r| c.players.get(*r))
        .map(|p| {
            columns
                .iter()
                .map(|d| {
                    render::value_of(p, d.code_name, owner)
                        .map(|v| render::user_image(d, v, bp_core::attribute::Look::LongText))
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    Some((format!("{} — {}", c.name, round.name), headers, rows))
}

/// Feuilles des poules retenues par `keep` (indice de la poule), avec
/// l'identité des tireurs selon les colonnes choisies pour la feuille.
/// `with_qr` : QR code de chaque feuille pour les téléphones des arbitres
/// (liaison SmartPoule réglée).
fn sheets_data(c: &Contest, index: usize, keep: impl Fn(usize) -> bool, for_referees: bool, with_qr: bool) -> Option<print::SheetsData> {
    let stage = c.schedule.stage(index)?.as_any().downcast_ref::<PoolSupervisorStage>()?;
    let round = stage.round();
    let kept: Vec<_> = round.pools().iter().enumerate().filter(|(i, _)| keep(*i)).map(|(_, p)| p).collect();
    let sheets: Vec<_> = kept.iter().map(|p| p.sheet(&c.players, c.referees.store(), for_referees)).collect();
    let qr: Vec<Option<String>> = kept
        .iter()
        .map(|p| if with_qr { crate::smartpoule::sheet_qr(&bp_core::smartpoule::pool_token(c.net_id, round.net_id(), p.number())) } else { None })
        .collect();
    let owner = round.supervisor_owner();
    let mut identities = std::collections::HashMap::new();
    for sheet in &sheets {
        for row in &sheet.rows {
            let Some(p) = c.players.get(row.player) else { continue };
            let mut identity = print::Identity::default();
            for code in &round.supervisor_filter {
                let Some(desc) = bp_core::attribute::registry().by_code(code) else { continue };
                let Some(value) = render::value_of(p, code, owner) else { continue };
                if desc.favorite_look == bp_core::attribute::Look::Graphical && desc.has_discrete_value() {
                    if let Some(icon) = render::discrete_icon(desc, value) {
                        identity.icons.push(icon);
                        continue;
                    }
                }
                identity.texts.push((code.clone(), render::user_image(desc, value, bp_core::attribute::Look::LongText)));
            }
            identities.insert(row.player, identity);
        }
    }
    Some(print::SheetsData {
        page_name: format!("{} - {}", tr(stage.kind().class_name()), round.name),
        sheets,
        qr,
        density: round.print_density(),
        identities,
    })
}

/// Résultats complets d'un tour de poules : feuilles de toutes les poules puis
/// classement cumulé (impression de l'événement).
pub fn results_sources(c: &Contest, index: usize) -> Vec<Box<dyn bp_gtk::print::PageSource>> {
    let mut parts: Vec<Box<dyn bp_gtk::print::PageSource>> = Vec::new();
    if let Some(data) = sheets_data(c, index, |_| true, false, false) {
        parts.push(print::sheets_source(&data, false));
    }
    if let Some((title, headers, rows)) = classification_data(c, index, false) {
        parts.push(print::classification_source(&format!("{title} — {}", tr("Ranking")), &headers, &rows));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use bp_core::stage::StageKind;

    /// Rend la première feuille de poule de `melun.cotcot` dans un PNG
    /// (`BP_SHEET_PNG`), pour contrôle visuel de la mise en page.
    #[test]
    fn render_pool_sheet_png() {
        let Some(out) = std::env::var_os("BP_SHEET_PNG") else { return };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        bp_core::contest::players::set_resources_dir(root.join("resources"));
        let _ = bp_core::i18n::set_language(&root.join("resources/translations"), "fr");
        bp_core::formats::discrete::init(&root.join("resources"), "fr");
        let mut contest = bp_core::contest::Contest::new(None, false);
        contest.load_file(&root.join("tests-data/melun.cotcot")).expect("melun");
        let c = &mut contest;
        let index = (0..c.schedule.len())
            .find(|i| c.schedule.stage(*i).map(|s| s.kind() == StageKind::PoolSupervisor).unwrap_or(false))
            .expect("poules");
        for referees in [false, true] {
            let mut data = sheets_data(c, index, |i| i == 0, referees, false).expect("feuilles");
            data.density = (1, 1);
            let header = bp_gtk::print::PageHeader {
                name: c.name.clone(),
                subtitle: format!("{} - {} - {}", c.weapon.label(), c.gender_label(), c.category.display_image()),
                organizer: c.organizer.clone(),
                date: c.date_image(),
                color: c.color.clone(),
            };
            let mut source = print::sheets_source(&data, false);
            let path = std::path::PathBuf::from(&out).join(if referees { "sheet_referees.png" } else { "sheet_results.png" });
            bp_gtk::print::export_png(source.as_mut(), Some(&header), 0, &path).expect("png");
        }
    }
}
