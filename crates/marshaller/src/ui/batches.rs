//! Onglets des compétitions et des lots (port de la partie graphique de
//! `competition.cpp`, `batch_panel.cpp`, `batch.cpp`, `job_board.cpp` et
//! `job_details.cpp`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use bp_core::i18n::tr;
use bp_gtk::{dialogs, glade::Glade};
use gtk::prelude::*;
use gtk4 as gtk;

use super::hall_view::{drag_source, Payload};
use super::Ui;
use crate::model::{BatchStatus, JobStatus};
use crate::slot::JobKey;

/// Vue d'un lot (`batch.glade`).
struct BatchWidgets {
    glade: Glade,
    root: gtk::Widget,
    store: gtk::ListStore,
    tree: gtk::TreeView,
    /// Matchs dans l'ordre des lignes.
    rows: Rc<RefCell<Vec<JobKey>>>,
    /// Panneau du lot dans l'onglet (bouton radio, avancement, état).
    radio: gtk::CheckButton,
    label: gtk::Label,
    progress: gtk::ProgressBar,
    status: gtk::Image,
    separator: gtk::Separator,
    updating: Rc<Cell<bool>>,
    /// « À placer : n · placés : m ».
    summary: gtk::Label,
    /// « Placer un groupe… » (entraînement des jeunes).
    group_button: gtk::Button,
}

// Colonnes de la liste des matchs.
const COL_ICON: u32 = 0;
const COL_NAME: u32 = 1;
const COL_BACKGROUND: u32 = 2;
const COL_GROUP: u32 = 3;
const COL_PISTE: u32 = 4;
const COL_TIME: u32 = 5;
const COL_REFEREES: u32 = 6;
const COL_STATE: u32 = 7;
const COL_STATE_COLOR: u32 = 8;
const COL_WEIGHT: u32 = 9;

/// Liste des matchs d'un lot : état bien visible (« À placer » d'abord),
/// groupe, piste, heure et arbitres.
fn setup_job_list(tree: &gtk::TreeView) -> gtk::ListStore {
    let s = String::static_type();
    let store = gtk::ListStore::new(&[s, s, s, s, s, s, s, s, s, i32::static_type()]);
    tree.set_model(Some(&store));
    for column in tree.columns() {
        tree.remove_column(&column);
    }
    tree.set_headers_visible(true);
    let text_column = |title: &str, col: u32, expand: bool| {
        let renderer = gtk::CellRendererText::new();
        let column = gtk::TreeViewColumn::new();
        column.set_title(title);
        column.pack_start(&renderer, true);
        column.add_attribute(&renderer, "text", col as i32);
        column.set_resizable(true);
        column.set_expand(expand);
        (column, renderer)
    };
    let column = gtk::TreeViewColumn::new();
    column.set_title(&tr("State"));
    let icon = gtk::CellRendererPixbuf::new();
    column.pack_start(&icon, false);
    column.add_attribute(&icon, "icon-name", COL_ICON as i32);
    let state = gtk::CellRendererText::new();
    column.pack_start(&state, true);
    column.add_attribute(&state, "text", COL_STATE as i32);
    column.add_attribute(&state, "foreground", COL_STATE_COLOR as i32);
    column.add_attribute(&state, "weight", COL_WEIGHT as i32);
    tree.append_column(&column);
    let (name, renderer) = text_column(&tr("Match"), COL_NAME, false);
    name.add_attribute(&renderer, "background", COL_BACKGROUND as i32);
    name.add_attribute(&renderer, "weight", COL_WEIGHT as i32);
    tree.append_column(&name);
    for (title, col, expand) in [(tr("Group"), COL_GROUP, false), (tr("Piste"), COL_PISTE, false), (tr("Time"), COL_TIME, false), (tr("Referees"), COL_REFEREES, true)] {
        let (column, _) = text_column(&title, col, expand);
        tree.append_column(&column);
    }
    store
}

/// Onglet d'une compétition (`competition.glade`).
struct CompetitionPage {
    glade: Glade,
    root: gtk::Widget,
    batches: HashMap<u32, BatchWidgets>,
    rows: i32,
}

pub struct BatchesView {
    ui: Weak<Ui>,
    notebook: gtk::Notebook,
    pages: RefCell<HashMap<u32, CompetitionPage>>,
}

impl BatchesView {
    pub fn new(ui: &Rc<Ui>, notebook: &gtk::Notebook) -> Rc<BatchesView> {
        Rc::new(BatchesView { ui: Rc::downgrade(ui), notebook: notebook.clone(), pages: RefCell::new(HashMap::new()) })
    }

    fn ui(&self) -> Option<Rc<Ui>> {
        self.ui.upgrade()
    }

    /// Réaffiche les onglets d'après le modèle (création, suppression,
    /// propriétés, lots et listes de matchs).
    pub fn refresh(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let ids: Vec<u32> = ui.hall.borrow().competitions.iter().map(|c| c.id).collect();

        // Compétitions disparues
        let gone: Vec<u32> = self.pages.borrow().keys().copied().filter(|k| !ids.contains(k)).collect();
        for id in gone {
            if let Some(page) = self.pages.borrow_mut().remove(&id) {
                if let Some(n) = self.notebook.page_num(&page.root) {
                    self.notebook.remove_page(Some(n));
                }
            }
        }
        // Nouvelles compétitions (ordre d'arrivée : `AttachTo` ajoute à la fin)
        for id in ids.iter().rev() {
            if !self.pages.borrow().contains_key(id) {
                self.add_page(*id);
            }
        }
        for id in &ids {
            self.refresh_page(*id);
        }
    }

    fn add_page(self: &Rc<Self>, id: u32) {
        let glade = Glade::new("competition.ui").expect("competition.ui");
        let root: gtk::Widget = glade.get("root");
        Glade::detach_from_parent(&root);
        let title: gtk::Widget = glade.get("notebook_title");
        Glade::detach_from_parent(&title);
        unsafe { root.set_data("competition", id) };
        self.notebook.append_page(&root, Some(&title));
        self.notebook.set_tab_reorderable(&root, true);
        // Bouton radio caché du groupe (`batch_group`).
        glade.get::<gtk::Widget>("batch_group").set_visible(false);
        self.pages.borrow_mut().insert(id, CompetitionPage { glade, root, batches: HashMap::new(), rows: 0 });
    }

    fn refresh_page(self: &Rc<Self>, id: u32) {
        let Some(ui) = self.ui() else { return };
        let Some((gender, weapon, category, color, frozen, batch_image, current, batches)) = ui.hall.borrow().competition(id).map(|c| {
            (
                c.gender.clone(),
                c.weapon_label.clone(),
                c.category.clone(),
                c.color,
                c.frozen,
                c.batch_image,
                c.current_batch,
                c.batches.iter().map(|b| b.id).collect::<Vec<_>>(),
            )
        }) else {
            return;
        };
        // Lots disparus
        {
            let mut pages = self.pages.borrow_mut();
            let Some(page) = pages.get_mut(&id) else { return };
            let gone: Vec<u32> = page.batches.keys().copied().filter(|b| !batches.contains(b)).collect();
            for b in gone {
                if let Some(w) = page.batches.remove(&b) {
                    for widget in [w.radio.upcast_ref::<gtk::Widget>(), w.label.upcast_ref(), w.progress.upcast_ref(), w.status.upcast_ref(), w.separator.upcast_ref(), &w.root] {
                        remove_from_parent(widget);
                    }
                }
            }
        }
        for b in &batches {
            let exists = self.pages.borrow().get(&id).map(|p| p.batches.contains_key(b)).unwrap_or(false);
            if !exists {
                self.add_batch(id, *b);
            }
        }

        let pages = self.pages.borrow();
        let Some(page) = pages.get(&id) else { return };
        let label = |w: &str, t: &str| page.glade.get::<gtk::Label>(w).set_text(&tr(t));
        label("competition_gender_label", &gender);
        label("competition_weapon_label", &weapon);
        label("competition_category_label", &category);
        let color_box: gtk::Widget = page.glade.get("color_box");
        set_background(&color_box, &color.to_css());
        page.glade.get::<gtk::Widget>("batch_image").set_visible(batch_image);
        page.glade.get::<gtk::Widget>("batch_table").set_visible(!frozen);
        page.glade.get::<gtk::Widget>("batch_vbox").set_visible(!frozen);

        for b in &batches {
            let Some(w) = page.batches.get(b) else { continue };
            self.refresh_batch(&ui, id, *b, w, current == Some(*b));
        }
    }

    fn add_batch(self: &Rc<Self>, c: u32, b: u32) {
        let Some(ui) = self.ui() else { return };
        let glade = Glade::new("batch.ui").expect("batch.ui");
        let root: gtk::Widget = glade.get("root");
        Glade::detach_from_parent(&root);
        let tree: gtk::TreeView = glade.get("competition_treeview");
        let store = setup_job_list(&tree);
        let toolbar: gtk::Box = glade.get("toolbar1");
        let group_button = gtk::Button::with_label(&tr("Place a group…"));
        group_button.set_tooltip_text(Some(&tr("Youth training: place all the pools of a group on one or several pistes")));
        group_button.set_visible(false);
        toolbar.append(&group_button);
        let summary = gtk::Label::new(None);
        summary.set_hexpand(true);
        summary.set_halign(gtk::Align::End);
        summary.set_margin_end(6);
        toolbar.append(&summary);
        tree.selection().set_mode(gtk::SelectionMode::Multiple);
        let rows: Rc<RefCell<Vec<JobKey>>> = Rc::new(RefCell::new(Vec::new()));

        let (name, ext1, ext2) = ui
            .hall
            .borrow()
            .batch(c, b)
            .map(|x| (x.name.clone(), x.extension1.clone(), x.extension2.clone()))
            .unwrap_or_default();
        match &ext1 {
            Some(e1) => {
                glade.get::<gtk::CheckButton>("extension1").set_label(Some(e1));
                glade.get::<gtk::Widget>("extension_viewport").set_visible(true);
                match &ext2 {
                    Some(e2) => {
                        glade.get::<gtk::CheckButton>("extension2").set_label(Some(e2));
                        glade.get::<gtk::Widget>("extension2").set_visible(true);
                    }
                    None => glade.get::<gtk::Widget>("extension2").set_visible(false),
                }
            }
            None => glade.get::<gtk::Widget>("extension_viewport").set_visible(false),
        }

        let updating = Rc::new(Cell::new(false));
        let weak = Rc::downgrade(self);
        let selected = {
            let tree = tree.clone();
            let rows = rows.clone();
            move || -> Vec<JobKey> {
                let (paths, _) = tree.selection().selected_rows();
                paths.iter().filter_map(|p| p.indices().first().and_then(|i| rows.borrow().get(*i as usize).copied())).collect()
            }
        };
        {
            let weak = weak.clone();
            let selected = selected.clone();
            group_button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade().and_then(|v| v.ui()) {
                    super::group_dialog::place_group(&ui, c, b, selected().first().copied());
                }
            });
        }
        {
            let weak = weak.clone();
            let selected = selected.clone();
            glade.connect_simple("on_assign_toolbutton_clicked", move |_| {
                if let Some(v) = weak.upgrade() {
                    if let Some(ui) = v.ui() {
                        if !ui.management_features().pistes {
                            let job = selected().first().copied().or_else(|| ui.hall.borrow().batch_pending_jobs(c, b).first().copied());
                            if let Some(job) = job { ui.edit_job_resources(job); }
                            return;
                        }
                        let hall_view = ui.hall_view.borrow().clone();
                        if let Some(h) = hall_view {
                            h.assign_batch(c, b, selected());
                        }
                    }
                }
            });
        }
        {
            let weak = weak.clone();
            glade.connect_simple("on_cancel_toolbutton_clicked", move |_| {
                if let Some(ui) = weak.upgrade().and_then(|v| v.ui()) {
                    ui.hall.borrow_mut().batch_assignment_cancel(c, b);
                    ui.after_change();
                }
            });
        }
        {
            let weak = weak.clone();
            let g = glade.clone();
            let updating = updating.clone();
            glade.connect_simple("on_extension_toggled", move |_| {
                if updating.get() {
                    return;
                }
                let choice = if g.get::<gtk::CheckButton>("extension1").is_active() {
                    1
                } else if g.get::<gtk::CheckButton>("extension2").is_active() {
                    2
                } else {
                    0
                };
                if let Some(ui) = weak.upgrade().and_then(|v| v.ui()) {
                    if ui.hall.borrow().batch(c, b).map(|x| x.extension_choice) != Some(choice) {
                        ui.hall.borrow_mut().batch_extension(c, b, choice);
                        ui.after_change();
                    }
                }
            });
        }
        {
            let weak = weak.clone();
            let rows = rows.clone();
            tree.connect_row_activated(move |_, path, _| {
                let Some(ui) = weak.upgrade().and_then(|v| v.ui()) else { return };
                let all = rows.borrow().clone();
                let job = path.indices().first().and_then(|i| all.get(*i as usize).copied());
                show_job_board(&ui, &all, job);
            });
        }
        // Source de glisser : premier match en attente sélectionné.
        {
            let weak = weak.clone();
            let selected = selected.clone();
            let rows_for_drag = rows.clone();
            drag_source(&tree, move |row| {
                let ui = weak.upgrade().and_then(|v| v.ui())?;
                let hall = ui.hall.borrow();
                let under = row.and_then(|r| rows_for_drag.borrow().get(r).copied());
                let candidates: Vec<JobKey> = match under {
                    Some(j) => vec![j],
                    None => selected(),
                };
                hall.batch_draggable_jobs(c, b, &candidates).first().map(|j| Payload::Job(*j))
            });
        }

        // Panneau du lot dans l'onglet (`BatchPanel`)
        let mut pages = self.pages.borrow_mut();
        let Some(page) = pages.get_mut(&c) else { return };
        let grid: gtk::Grid = page.glade.get("batch_table");
        let group: gtk::CheckButton = page.glade.get("batch_group");
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        let row = page.rows;
        if row >= 1 {
            grid.attach(&separator, 0, row, 3, 1);
        }
        let radio = gtk::CheckButton::with_label(&name);
        radio.set_group(Some(&group));
        let label = gtk::Label::new(Some("Full"));
        let progress = gtk::ProgressBar::new();
        progress.set_size_request(50, 5);
        let status = gtk::Image::from_icon_name("dialog-question-symbolic");
        grid.attach(&radio, 0, row + 1, 1, 2);
        grid.attach(&label, 1, row + 1, 1, 1);
        grid.attach(&progress, 1, row + 2, 1, 1);
        grid.attach(&status, 2, row, 1, 2);
        page.rows += 3;
        {
            let weak = weak.clone();
            let updating = updating.clone();
            radio.connect_toggled(move |r| {
                if r.is_active() && !updating.get() {
                    if let Some(ui) = weak.upgrade().and_then(|v| v.ui()) {
                        if let Some(comp) = ui.hall.borrow_mut().competition_mut(c) {
                            comp.current_batch = Some(b);
                        }
                        let weapon = ui.hall.borrow().competition(c).map(|x| x.weapon.clone()).unwrap_or_default();
                        ui.expose_weapon(&weapon);
                        ui.refresh();
                    }
                }
            });
        }
        page.glade.get::<gtk::Box>("batch_vbox").append(&root);
        root.set_visible(false);
        page.batches.insert(b, BatchWidgets { glade, root, store, tree, rows, radio, label, progress, status, separator, updating, summary, group_button });
    }

    fn refresh_batch(&self, ui: &Rc<Ui>, c: u32, b: u32, w: &BatchWidgets, current: bool) {
        let hall = ui.hall.borrow();
        let Some(batch) = hall.batch(c, b) else { return };
        w.updating.set(true);

        // Panneau
        let visible = batch.panel.visible;
        for widget in [w.radio.upcast_ref::<gtk::Widget>(), w.label.upcast_ref(), w.progress.upcast_ref(), w.separator.upcast_ref()] {
            widget.set_visible(visible);
        }
        w.status.set_visible(visible && batch.status == BatchStatus::Incomplete);
        let (expected, ready) = (batch.panel.expected, batch.panel.ready);
        let weight = if expected == ready { " font_weight=\"bold\"" } else { "" };
        w.label.set_markup(&format!("<span size=\"small\"{weight}>{ready}/{expected}</span>"));
        w.progress.set_fraction(if expected > 0 { ready as f64 / expected as f64 } else { 0.0 });
        if current && !w.radio.is_active() {
            w.radio.set_active(true);
        }
        w.root.set_visible(current && visible);

        // Boutons
        if ui.local_output.is_some() {
            let caption = if ui.management_features().pistes { tr("Assign") } else { tr("Assign referees") };
            let button = w.glade.get::<gtk::Button>("assign_toolbutton");
            if button.label().as_deref() != Some(caption.as_str()) { button.set_label(&caption); }
        }
        w.glade.get::<gtk::Widget>("assign_toolbutton").set_sensitive(batch.assign_sensitive);
        w.glade.get::<gtk::Widget>("cancel_toolbutton").set_sensitive(batch.cancel_sensitive);
        let ext = match batch.extension_choice {
            1 => "extension1",
            2 => "extension2",
            _ => "radiobutton1",
        };
        w.glade.get::<gtk::CheckButton>(ext).set_active(true);

        // Liste des matchs (sélection conservée) : à placer d'abord.
        let mut ordered: Vec<&crate::model::JobRow> = batch.rows.iter().collect();
        ordered.sort_by_key(|r| hall.jobs.get(&r.job).map(|j| j.slot.is_some()).unwrap_or(true));
        let jobs: Vec<JobKey> = ordered.iter().map(|r| r.job).collect();
        let (paths, _) = w.tree.selection().selected_rows();
        let selected: Vec<JobKey> = paths
            .iter()
            .filter_map(|p| p.indices().first().and_then(|i| w.rows.borrow().get(*i as usize).copied()))
            .collect();
        w.store.clear();
        let mut pending = 0;
        for row in &ordered {
            let Some(job) = hall.jobs.get(&row.job) else { continue };
            let icon = match row.status {
                JobStatus::None => "",
                JobStatus::Question => "dialog-question-symbolic",
                JobStatus::Error => "dialog-error-symbolic",
            };
            let slot = job.slot.and_then(|sk| hall.slots.get(&sk));
            let pistes = slot
                .map(|s| s.pistes().iter().filter_map(|k| hall.piste(*k)).map(|p| p.id.to_string()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let time = slot.map(|s| s.start.format("%H:%M").to_string()).unwrap_or_default();
            let referees = slot
                .map(|s| s.referees.iter().filter_map(|r| hall.referees.get(*r)).map(|r| r.player.name()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let (state, color, weight) = if slot.is_none() {
                pending += 1;
                (tr("To place"), "#b45309", 700)
            } else if pistes.is_empty() {
                (tr("Referees only"), "#1f5ae0", 400)
            } else {
                (tr("Placed"), "#15803d", 400)
            };
            let background = row.warning.map(|c| c.to_css()).unwrap_or_else(|| if slot.is_none() { "#fff7e0".to_string() } else { "#ffffff".to_string() });
            let group = job.group.map(|g| g.to_string()).unwrap_or_default();
            let iter = w.store.append();
            w.store.set(
                &iter,
                &[
                    (COL_ICON, &icon),
                    (COL_NAME, &job.name),
                    (COL_BACKGROUND, &background),
                    (COL_GROUP, &group),
                    (COL_PISTE, &pistes),
                    (COL_TIME, &time),
                    (COL_REFEREES, &referees),
                    (COL_STATE, &state),
                    (COL_STATE_COLOR, &color),
                    (COL_WEIGHT, &weight),
                ],
            );
        }
        let placed = jobs.len() - pending;
        w.summary.set_markup(&format!(
            "<b>{}</b> {}  ·  {} {}",
            pending,
            gtk::glib::markup_escape_text(&tr("to place")),
            placed,
            gtk::glib::markup_escape_text(&tr("placed"))
        ));
        let has_groups = !hall.batch_groups(c, b).is_empty();
        w.group_button.set_visible(has_groups);
        if let Some(column) = w.tree.column(2) {
            column.set_visible(has_groups);
        }
        *w.rows.borrow_mut() = jobs.clone();
        for (i, j) in jobs.iter().enumerate() {
            if selected.contains(j) {
                w.tree.selection().select_path(&gtk::TreePath::from_indices(&[i as i32]));
            }
        }
        w.updating.set(false);
    }
}

/// Retire un widget de sa grille ou de sa boîte.
fn remove_from_parent(widget: &gtk::Widget) {
    match widget.parent() {
        Some(p) => {
            if let Some(grid) = p.downcast_ref::<gtk::Grid>() {
                grid.remove(widget);
            } else if let Some(b) = p.downcast_ref::<gtk::Box>() {
                b.remove(widget);
            } else {
                widget.unparent();
            }
        }
        None => {}
    }
}

fn set_background(widget: &gtk::Widget, css_color: &str) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!("* {{ background-color: {css_color}; }}"));
    #[allow(deprecated)]
    widget.style_context().add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

// ------------------------------------------------------------------------------------
// Tableau des matchs (`JobBoard`)

thread_local! {
    static BOARD: RefCell<Option<Rc<JobBoard>>> = const { RefCell::new(None) };
}

struct JobBoard {
    ui: Weak<Ui>,
    glade: Glade,
    jobs: RefCell<Vec<JobKey>>,
    current: Cell<usize>,
    /// Heure du match saisie à la main (« HH:MM »).
    time_entry: gtk::Entry,
}

/// `JobBoard::Display` : affiche le match `job` (sinon celui en cours à
/// l'heure du curseur) parmi `jobs`.
pub fn show_job_board(ui: &Rc<Ui>, jobs: &[JobKey], job: Option<JobKey>) {
    let board = BOARD.with(|b| b.borrow().clone()).unwrap_or_else(|| {
        let glade = Glade::new("job_board.ui").expect("job_board.ui");
        let time_entry = gtk::Entry::new();
        let board = Rc::new(JobBoard { ui: Rc::downgrade(ui), glade, jobs: RefCell::new(Vec::new()), current: Cell::new(0), time_entry });
        board.connect();
        BOARD.with(|b| *b.borrow_mut() = Some(board.clone()));
        board
    });
    *board.jobs.borrow_mut() = jobs.to_vec();
    let index = match job {
        Some(j) => jobs.iter().position(|x| *x == j).unwrap_or(0),
        None => {
            let hall = ui.hall.borrow();
            let cursor = hall.timeline.cursor_time(false);
            jobs.iter()
                .position(|j| {
                    hall.jobs.get(j).and_then(|x| x.slot).and_then(|s| hall.slots.get(&s)).map(|s| s.time_is_inside(cursor)).unwrap_or(false)
                })
                .unwrap_or(0)
        }
    };
    board.current.set(index);
    board.display();
    let dialog: gtk::Dialog = board.glade.get("dialog");
    dialogs::raise(&dialog, None);
}

impl JobBoard {
    fn connect(self: &Rc<Self>) {
        // Heure du match réglable à la main, à côté de l'heure affichée.
        {
            let entry = &self.time_entry;
            entry.set_width_chars(5);
            entry.set_max_width_chars(5);
            entry.set_placeholder_text(Some("HH:MM"));
            entry.set_tooltip_text(Some(&tr("Start time of the bout (HH:MM)")));
            let apply = gtk::Button::with_label(&tr("Set the time"));
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.set_margin_top(4);
            row.append(&gtk::Label::new(Some(&tr("Time:"))));
            row.append(entry);
            row.append(&apply);
            let label: gtk::Widget = self.glade.get("time_label");
            match label.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) {
                Some(parent) => parent.append(&row),
                None => self.glade.get::<gtk::Dialog>("dialog").content_area().append(&row),
            }
            let weak = Rc::downgrade(self);
            let go = move || {
                if let Some(board) = weak.upgrade() {
                    board.apply_time();
                }
            };
            let g2 = go.clone();
            apply.connect_clicked(move |_| g2());
            entry.connect_activate(move |_| go());
        }
        let resources = gtk::Button::with_label(&tr("Resources…"));
        resources.set_widget_name("job-resources");
        self.glade.get::<gtk::Dialog>("dialog").add_action_widget(&resources, gtk::ResponseType::Other(301));
        let weak = Rc::downgrade(self);
        resources.connect_clicked(move |_| {
            if let Some(board) = weak.upgrade() {
                if let (Some(ui), Some(job)) = (board.ui.upgrade(), board.current_job()) {
                    ui.edit_job_resources(job); board.display();
                }
            }
        });
        // Désaffecter : le match quitte sa piste, ou ses arbitres le quittent.
        for (label, tooltip, take_off) in [
            (tr("Take off the piste"), tr("The bout (or pool) and its referees leave the piste"), true),
            (tr("Remove the referees"), tr("The referees leave the bout, which stays on its piste"), false),
        ] {
            let button = gtk::Button::with_label(&label);
            button.set_tooltip_text(Some(&tooltip));
            self.glade.get::<gtk::Dialog>("dialog").add_action_widget(&button, gtk::ResponseType::Other(if take_off { 302 } else { 303 }));
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(board) = weak.upgrade() else { return };
                let (Some(ui), Some(job)) = (board.ui.upgrade(), board.current_job()) else { return };
                {
                    let mut hall = ui.hall.borrow_mut();
                    if take_off {
                        hall.job_take_off(job);
                    } else {
                        hall.job_remove_referees(job);
                    }
                }
                ui.after_change();
                if take_off {
                    board.glade.get::<gtk::Dialog>("dialog").set_visible(false);
                } else {
                    board.display();
                }
            });
        }
        macro_rules! on {
            ($name:expr, $body:expr) => {{
                let w = Rc::downgrade(self);
                let f = $body;
                self.glade.connect_simple($name, move |_| {
                    if let Some(b) = w.upgrade() {
                        f(&b);
                    }
                });
            }};
        }
        on!("on_previous_job_clicked", |b: &Rc<JobBoard>| {
            b.current.set(b.current.get().saturating_sub(1));
            b.display();
        });
        on!("on_next_job_clicked", |b: &Rc<JobBoard>| {
            let n = b.jobs.borrow().len();
            b.current.set((b.current.get() + 1).min(n.saturating_sub(1)));
            b.display();
        });
        on!("on_take_off_button_clicked", |b: &Rc<JobBoard>| b.take_off());
        on!("on_move_button_clicked", |b: &Rc<JobBoard>| b.move_job());
        on!("on_close_board_button_clicked", |b: &Rc<JobBoard>| b.close());
        on!("on_dialog_delete_event", |b: &Rc<JobBoard>| b.close());
        let dialog: gtk::Dialog = self.glade.get("dialog");
        let w = Rc::downgrade(self);
        dialog.connect_close_request(move |_| {
            if let Some(b) = w.upgrade() {
                b.close();
            }
            glib::Propagation::Stop
        });
    }

    /// Change l'heure du match (`Hall::set_job_start`).
    fn apply_time(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let Some(job) = self.current_job() else { return };
        let text = self.time_entry.text().to_string();
        let parsed = text
            .trim()
            .split_once(|c| c == ':' || c == 'h' || c == 'H')
            .and_then(|(h, m)| Some((h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().unwrap_or(0))))
            .filter(|(h, m)| *h < 24 && *m < 60);
        let Some((h, m)) = parsed else {
            dialogs::error(Some(ui.window()), &tr("Enter the time as HH:MM."));
            return;
        };
        let base = {
            let hall = ui.hall.borrow();
            hall.jobs
                .get(&job)
                .and_then(|j| j.slot)
                .and_then(|s| hall.slots.get(&s))
                .map(|s| s.start)
                .unwrap_or_else(|| hall.timeline.cursor_time(true))
        };
        use chrono::Timelike;
        let Some(start) = base.with_hour(h).and_then(|t| t.with_minute(m)).and_then(|t| t.with_second(0)) else { return };
        let result = ui.hall.borrow_mut().set_job_start(job, start);
        match result {
            Ok(()) => {
                ui.after_change();
                self.display();
            }
            Err(e) => dialogs::error(Some(ui.window()), &e),
        }
    }

    fn current_job(&self) -> Option<JobKey> {
        self.jobs.borrow().get(self.current.get()).copied()
    }

    fn close(&self) {
        self.glade.get::<gtk::Widget>("dialog").set_visible(false);
    }

    /// `OnTakeOffClicked` : le match quitte sa piste.
    fn take_off(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        if let Some(job) = self.current_job() {
            let slot = ui.hall.borrow().jobs.get(&job).and_then(|j| j.slot);
            if let Some(sk) = slot {
                let mut hall = ui.hall.borrow_mut();
                hall.slot_remove_job(sk, job);
                hall.gc_slots();
                hall.redraw();
            }
            ui.after_change();
        }
        self.close();
    }

    /// `OnMoveClicked` : le match suit la souris jusqu'à une piste.
    fn move_job(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.management_features().pistes { return; }
        if let Some(job) = self.current_job() {
            ui.hall.borrow_mut().start_moving_job(job);
            ui.after_change();
        }
        self.close();
    }

    /// `DisplayCurrent` + `SetHeader`.
    fn display(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let Some(job) = self.current_job() else { return };
        let g = &self.glade;
        let (name, slot, competition, batch_id) = {
            let hall = ui.hall.borrow();
            let Some(j) = hall.jobs.get(&job) else { return };
            (j.name.clone(), j.slot.and_then(|s| hall.slots.get(&s).cloned()), j.competition, j.batch)
        };
        g.get::<gtk::Label>("current_job_label").set_text(&name);
        match &slot {
            Some(s) => {
                let pistes = s.pistes().iter().filter_map(|p| ui.hall.borrow().piste(*p).map(|x| x.id.to_string())).collect::<Vec<_>>().join(", ");
                g.get::<gtk::Label>("piste_label").set_text(&tr("Piste %d").replace("%d", &pistes));
                g.get::<gtk::Label>("time_label").set_text(&s.start.format("%H:%M").to_string());
                self.time_entry.set_text(&s.start.format("%H:%M").to_string());
                {
                    let mut hall = ui.hall.borrow_mut();
                    hall.timeline.set_cursor_time(s.start);
                    hall.on_timeline_cursor_moved();
                    if let Some(p) = s.piste {
                        hall.cancel_selection();
                        hall.select_piste(p);
                    }
                }
            }
            None => {
                g.get::<gtk::Label>("piste_label").set_text(&tr("No piste!"));
                g.get::<gtk::Label>("time_label").set_text(&tr("No time!"));
                self.time_entry.set_text("");
            }
        }
        let modifiable = ui.hall.borrow().batch(competition, batch_id).map(|b| b.panel.visible).unwrap_or(false);
        if let Some(button) = g.get::<gtk::Dialog>("dialog").widget_for_response(gtk::ResponseType::Other(301)) {
            let eligible = ui.management_features().referees || (ui.management_features().pistes && ui.hall.borrow().jobs.get(&job).is_some_and(|j| j.is_pool));
            button.set_sensitive(modifiable && eligible);
        }
        g.get::<gtk::Widget>("move_button").set_sensitive(modifiable && slot.is_some() && ui.management_features().pistes);
        g.get::<gtk::Widget>("take_off_button").set_sensitive(modifiable && slot.is_some());

        // En-tête
        if let Some(c) = ui.hall.borrow().competition(competition) {
            g.get::<gtk::Label>("weapon_label").set_text(&tr(&c.weapon_label));
            g.get::<gtk::Label>("gender_label").set_text(&tr(&c.gender));
            g.get::<gtk::Label>("category_label").set_text(&tr(&c.category));
            set_background(&g.get::<gtk::Widget>("header_box"), &c.color.to_css());
        }
        let n = self.jobs.borrow().len();
        g.get::<gtk::Widget>("previous_job").set_sensitive(self.current.get() > 0);
        g.get::<gtk::Widget>("next_job").set_sensitive(self.current.get() + 1 < n);

        // Arbitres et tireurs (`JobDetails`) : couleur d'affinité.
        let referees_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let fencers_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        {
            let hall = ui.hall.borrow();
            let j = hall.jobs.get(&job).unwrap();
            let referees: Vec<u32> = slot.as_ref().map(|s| s.referees.clone()).unwrap_or_default();
            for r in &referees {
                let Some(referee) = hall.referees.get(*r) else { continue };
                let kinship = hall.job_kinship_with(job, *r);
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let label = detail_label(&format!("{} {}", referee.player.name(), referee.player.first_name()), &club_of(&referee.player));
                if let Some(c) = hall.affinities.color_of_kinship(kinship) {
                    set_background(label.upcast_ref(), &c.to_css());
                }
                row.append(&label);
                if modifiable {
                    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
                    remove.set_has_frame(false);
                    remove.set_tooltip_text(Some(&tr("Remove")));
                    let w = Rc::downgrade(self);
                    let referee_ref = *r;
                    remove.connect_clicked(move |_| {
                        let Some(b) = w.upgrade() else { return };
                        let Some(ui) = b.ui.upgrade() else { return };
                        let sk = ui.hall.borrow().jobs.get(&job).and_then(|j| j.slot);
                        if let Some(sk) = sk {
                            ui.hall.borrow_mut().slot_remove_referee(sk, referee_ref);
                            ui.hall.borrow_mut().refresh_referee_workloads(referee_ref);
                        }
                        ui.after_change();
                        b.display();
                    });
                    row.append(&remove);
                }
                referees_box.append(&row);
            }
            for f in &j.fencers {
                let worst = referees.iter().map(|r| hall.referees.get(*r).map(|x| x.affinities.kinship_with(Some(&f.affinities), &hall.affinities)).unwrap_or(0)).fold(0, |a, b| a | b);
                let label = detail_label(&format!("{} {}", f.player.name(), f.player.first_name()), &club_of(&f.player));
                if let Some(c) = hall.affinities.color_of_kinship(worst) {
                    set_background(label.upcast_ref(), &c.to_css());
                }
                fencers_box.append(&label);
            }
        }
        g.get::<gtk::Viewport>("referee_detail_hook").set_child(Some(&referees_box));
        g.get::<gtk::Viewport>("fencer_detail_hook").set_child(Some(&fencers_box));
        ui.refresh();
    }
}

fn club_of(p: &bp_core::Player) -> String {
    ["club", "league", "country"]
        .iter()
        .filter_map(|c| p.str_attr(c))
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn detail_label(name: &str, details: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.set_markup(&format!(
        "<b>{}</b>  <span foreground=\"dimgrey\" style=\"italic\">{}</span>",
        gtk::glib::markup_escape_text(name),
        gtk::glib::markup_escape_text(details)
    ));
    label
}

use gtk::glib;
