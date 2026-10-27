//! Calendrier des phases d'une compétition (port des parties interface de
//! `supervisor/schedule.cpp`) : liste de la formule, boutons précédent /
//! suivant, erreur de la phase courante et onglets des phases.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use bp_core::i18n::tr;
use bp_core::rounds::checkin::CheckinStage;
use bp_core::rounds::pool::PoolAllocatorStage;
use bp_core::rounds::table::supervisor::{FencedPlaces, TableSupervisor};
use bp_core::stage::{Stage, StageKind};
use bp_gtk::dialogs;
use bp_gtk::glade::Glade;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::contest_page::ContestPage;
use crate::stages::{self, StageScreen};
use crate::widgets::list_document::ListDocument;
use crate::widgets::players_list::{Cell as ListCell, CellKind, ColumnSpec, RowData};

/// Colonnes de `stage_liststore`.
const COL_NAME: u32 = 0;
const COL_LOCK: u32 = 1;
const COL_VISIBLE: u32 = 3;

type ScreenSlot = Option<(StageKind, Rc<dyn StageScreen>)>;

pub struct ScheduleView {
    page: Weak<ContestPage>,
    glade: Glade,
    store: gtk::ListStore,
    filter: gtk::TreeModelFilter,
    notebook: gtk::Notebook,
    screens: RefCell<Vec<ScreenSlot>>,
    /// Les modifications de la sélection venant du code ne déclenchent rien.
    updating: std::cell::Cell<bool>,
}

impl ScheduleView {
    pub fn new(page: &Rc<ContestPage>) -> Rc<ScheduleView> {
        let glade = Glade::new("schedule.ui").expect("schedule.ui");
        let store: gtk::ListStore = glade.get("stage_liststore");
        let filter: gtk::TreeModelFilter = glade.get("stage_liststore_filter");
        filter.set_visible_column(COL_VISIBLE as i32);
        let notebook: gtk::Notebook = glade.get("schedule_notebook");

        let view = Rc::new(ScheduleView {
            page: Rc::downgrade(page),
            glade,
            store,
            filter,
            notebook,
            screens: RefCell::new(Vec::new()),
            updating: std::cell::Cell::new(false),
        });

        // Les boutons de navigation rejoignent la barre d'outils de la compétition.
        if let Some(toolbar) = page.glade.widget("contest_toolbar") {
            for id in [
                "previous_stage_toolbutton",
                "next_stage_toolbutton",
                "error_toolbutton",
            ] {
                if let Some(w) = view.glade.widget(id) {
                    Glade::detach_from_parent(&w);
                    bp_gtk::glade::container_add(&toolbar, &w);
                }
            }
        }
        view.connect_handlers();
        let weak_page = Rc::downgrade(page);
        view.notebook.connect_switch_page(move |_, _, _| {
            let weak_page = weak_page.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(page) = weak_page.upgrade() {
                    if let Some(app) = page.app.upgrade() {
                        let shell = app.shell.borrow().clone();
                        if let Some(shell) = shell { shell.refresh_navigation(); }
                    }
                }
            });
        });
        view.build_add_menu();
        view
    }

    /// Carnet des phases.
    pub fn root(&self) -> gtk::Widget {
        let w: gtk::Widget = self.glade.get("vbox1");
        Glade::detach_from_parent(&w);
        w
    }

    /// Liste de la formule et paramètres de la phase.
    pub fn config_widget(&self) -> gtk::Widget {
        let w: gtk::Widget = self.glade.get("config_vbox");
        Glade::detach_from_parent(&w);
        w
    }

    fn stage_widget(&self, kinds: &[StageKind]) -> Option<gtk::Widget> {
        let screens = self.screens.borrow();
        kinds.iter().find_map(|kind| screens.iter().rev().flatten()
            .find(|(candidate, screen)| candidate == kind && self.notebook.page_num(&screen.widget()).is_some())
            .map(|(_, screen)| screen.widget()))
    }

    pub fn has_visible_stage(&self, kinds: &[StageKind]) -> bool { self.stage_widget(kinds).is_some() }

    /// Navigation d'affichage uniquement : aucune phase n'est avancée ou déverrouillée.
    pub fn show_stage(&self, kinds: &[StageKind]) -> bool {
        let Some(widget) = self.stage_widget(kinds) else { return false };
        self.notebook.set_current_page(self.notebook.page_num(&widget));
        true
    }

    /// Sélectionne dans la formule la première phase de ce type et affiche ses
    /// préférences (captures de la documentation).
    pub fn select_stage_kind(self: &Rc<Self>, kind: StageKind) {
        let Some(page) = self.page() else { return };
        let index = page.with_contest(|c| c.schedule.stages().position(|s| s.kind() == kind)).flatten();
        if let Some(i) = index {
            self.select_index(i);
            self.refresh_sensitivity();
            self.show_configuration();
        }
    }

    pub fn visible_stage_kind(&self) -> Option<StageKind> {
        let current = self.notebook.current_page()?;
        self.screens.borrow().iter().flatten()
            .find(|(_, screen)| self.notebook.page_num(&screen.widget()) == Some(current))
            .map(|(kind, _)| *kind)
    }

    fn connect_handlers(self: &Rc<Self>) {
        macro_rules! on {
            ($name:expr, $method:ident) => {{
                let w = Rc::downgrade(self);
                self.glade.connect_simple($name, move |_| {
                    if let Some(v) = w.upgrade() {
                        v.$method();
                    }
                });
            }};
        }
        on!("on_previous_stage_toolbutton_clicked", on_previous);
        on!("on_next_stage_toolbutton_clicked", on_next);
        on!("on_remove_stage_toolbutton_clicked", on_remove);
        on!("on_move_stage_up_toolbutton_clicked", on_move_up);
        on!("on_move_stage_down_toolbutton_clicked", on_move_down);
        on!("on_formula_treeview_cursor_changed", on_stage_selected);
        on!("on_print_schedule_toolbutton_clicked", on_print);
    }

    /// Menu « Ajouter » : les phases modifiables et supprimables (`Schedule`).
    fn build_add_menu(self: &Rc<Self>) {
        let button: gtk::MenuButton = self.glade.get("add_stage_toolbutton");
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for kind in StageKind::ALL {
            let rights = kind.rights();
            if !(rights.editable && rights.removable) {
                continue;
            }
            let item = gtk::Button::with_label(&tr(kind.class_name()));
            item.set_has_frame(false);
            let w = Rc::downgrade(self);
            let pop = popover.clone();
            item.connect_clicked(move |_| {
                pop.popdown();
                if let Some(v) = w.upgrade() {
                    v.add_stage(kind);
                }
            });
            list.append(&item);
        }
        popover.set_child(Some(&list));
        button.set_popover(Some(&popover));
    }

    // ----------------------------------------------------------------------

    fn page(&self) -> Option<Rc<ContestPage>> {
        self.page.upgrade()
    }

    /// Index (dans la suite complète des phases) de la phase sélectionnée dans la formule.
    fn selected_index(&self) -> Option<usize> {
        let tree: gtk::TreeView = self.glade.get("formula_treeview");
        let (model, iter) = tree.selection().selected()?;
        let _ = model;
        let child = self.filter.convert_iter_to_child_iter(&iter);
        self.store
            .path(&child)
            .indices()
            .first()
            .map(|i| *i as usize)
    }

    fn select_index(&self, index: usize) {
        let tree: gtk::TreeView = self.glade.get("formula_treeview");
        let path = gtk::TreePath::from_indices(&[index as i32]);
        if let Some(fp) = self.filter.convert_child_path_to_path(&path) {
            self.updating.set(true);
            tree.selection().select_path(&fp);
            self.updating.set(false);
        }
    }

    /// Réaffiche le contenu des écrans des phases (données reçues du
    /// réseau), sans changer d'onglet.
    pub fn refresh_screens(&self) {
        let screens: Vec<_> = self.screens.borrow().iter().flatten().map(|(_, s)| s.clone()).collect();
        for screen in screens {
            screen.refresh();
        }
    }

    /// Recompose la formule, les onglets et les boutons d'après le modèle.
    pub fn refresh(self: &Rc<Self>) {
        let Some(page) = self.page() else { return };
        let Some(snapshot) = page.with_contest(|c| {
            let current = c.schedule.current_index();
            let stages: Vec<(StageKind, String, String, bool)> = c
                .schedule
                .stages()
                .map(|s| {
                    (
                        s.kind(),
                        tr(s.kind().class_name()),
                        s.name(),
                        s.base().locked,
                    )
                })
                .collect();
            (current, stages)
        }) else {
            return;
        };
        let (current, stages) = snapshot;
        let selected = self.selected_index();

        // Formule
        self.updating.set(true);
        self.store.clear();
        for (kind, purpose, _, locked) in &stages {
            let iter = self.store.append();
            let lock = if *locked { "dialog-password-symbolic" } else { "" };
            self.store.set(
                &iter,
                &[
                    (COL_NAME, purpose),
                    (COL_LOCK, &lock),
                    (COL_VISIBLE, &kind.rights().editable),
                ],
            );
        }
        self.updating.set(false);
        self.select_index(selected.filter(|i| *i < stages.len()).unwrap_or(current));

        // Onglets : les phases jusqu'à la phase courante. Aucun emprunt de
        // la liste n'est gardé pendant la construction d'un écran (la fenêtre
        // d'attente laisse tourner la boucle d'événements entre deux écrans).
        {
            let mut screens = self.screens.borrow_mut();
            for (_, screen) in screens.iter().skip(stages.len()).flatten() {
                if let Some(n) = self.notebook.page_num(&screen.widget()) {
                    self.notebook.remove_page(Some(n));
                }
            }
            screens.resize_with(stages.len(), || None);
        }
        for (i, (kind, purpose, name, _)) in stages.iter().enumerate() {
            // L'absence de balise XML n'enlève pas l'écran : l'allocateur
            // et le superviseur ont deux onglets, comme Schedule::PlugStage.
            let existing = {
                let mut screens = self.screens.borrow_mut();
                if screens[i].as_ref().is_some_and(|(old_kind, _)| old_kind != kind) {
                    if let Some((_, screen)) = screens[i].take() {
                        if let Some(n) = self.notebook.page_num(&screen.widget()) {
                            self.notebook.remove_page(Some(n));
                        }
                    }
                }
                screens[i].clone()
            };
            let shown = i <= current;
            match (shown, existing) {
                (true, None) => {
                    let screen = stages::build(&page, i);
                    let label = gtk::Label::new(Some(&format!("{purpose}\n{name}")));
                    label.set_justify(gtk::Justification::Center);
                    self.notebook.append_page(&screen.widget(), Some(&label));
                    self.screens.borrow_mut()[i] = Some((*kind, screen));
                    crate::loading::step(purpose);
                }
                (true, Some((_, screen))) => {
                    if self.notebook.page_num(&screen.widget()).is_none() {
                        let label = gtk::Label::new(Some(&format!("{purpose}\n{name}")));
                        self.notebook.append_page(&screen.widget(), Some(&label));
                    }
                    if let Some(l) = self
                        .notebook
                        .tab_label(&screen.widget())
                        .and_then(|l| l.downcast::<gtk::Label>().ok())
                    {
                        l.set_text(&format!("{purpose}\n{name}"));
                    }
                    screen.refresh();
                }
                (false, Some((_, screen))) => {
                    if let Some(n) = self.notebook.page_num(&screen.widget()) {
                        self.notebook.remove_page(Some(n));
                    }
                    // Garder les paramètres d'une phase future disponibles.
                }
                (false, None) => {}
            }
        }
        {
            let screens = self.screens.borrow();
            for (i, (_, screen)) in screens.iter().flatten().enumerate() {
                if self.notebook.page_num(&screen.widget()).is_some() {
                    self.notebook
                        .reorder_child(&screen.widget(), Some(i as u32));
                }
            }
            if let Some(Some((_, screen))) = screens.get(current) {
                if let Some(n) = self.notebook.page_num(&screen.widget()) {
                    self.notebook.set_current_page(Some(n));
                }
            }
        }
        self.refresh_sensitivity();
        self.show_configuration();
        page.refresh_header();
    }

    /// `Schedule::RefreshSensitivity` et état des boutons d'édition.
    pub fn refresh_sensitivity(&self) {
        let Some(page) = self.page() else { return };
        let state = page.with_contest(|c| {
            let sensitivity = c.with_schedule(|s, ctx| s.sensitivity(ctx));
            let error = c.with_schedule(|s, ctx| s.current_error(ctx));
            let current = c.schedule.current_index();
            (sensitivity, error, current, c.schedule.len())
        });
        let Some((sensitivity, error, current, count)) = state else {
            return;
        };
        self.glade
            .get::<gtk::Widget>("previous_stage_toolbutton")
            .set_sensitive(sensitivity.previous);
        self.glade
            .get::<gtk::Widget>("next_stage_toolbutton")
            .set_sensitive(sensitivity.next && current + 1 < count);

        let error_button: gtk::Widget = self.glade.get("error_toolbutton");
        match error {
            Some(e) => {
                error_button.set_visible(true);
                let text = match &e.guilty {
                    Some(g) => format!(
                        "<b>{}</b>\n{}",
                        gtk::glib::markup_escape_text(g),
                        gtk::glib::markup_escape_text(&e.message)
                    ),
                    None => gtk::glib::markup_escape_text(&e.message).to_string(),
                };
                self.glade
                    .get::<gtk::Label>("error_label")
                    .set_markup(&text);
            }
            None => error_button.set_visible(false),
        }

        // Boutons « Ajouter » / « Supprimer » selon la phase sélectionnée
        let selected = self.selected_index();
        let (can_add, can_remove, can_up, can_down) = page
            .with_contest(|c| match selected {
                Some(i) => (c.schedule.can_add_after(i), c.schedule.can_remove(i), c.schedule.can_move(i, true), c.schedule.can_move(i, false)),
                None => (true, false, false, false),
            })
            .unwrap_or((false, false, false, false));
        self.glade
            .get::<gtk::Widget>("add_stage_toolbutton")
            .set_sensitive(can_add);
        self.glade
            .get::<gtk::Widget>("remove_stage_toolbutton")
            .set_sensitive(can_remove);
        self.glade
            .get::<gtk::Widget>("move_stage_up_toolbutton")
            .set_sensitive(can_up);
        self.glade
            .get::<gtk::Widget>("move_stage_down_toolbutton")
            .set_sensitive(can_down);
    }

    // ----------------------------------------------------------------------
    // Actions

    fn on_stage_selected(self: &Rc<Self>) {
        if self.updating.get() {
            return;
        }
        self.refresh_sensitivity();
        self.show_configuration();
    }

    fn show_configuration(&self) {
        let hook: gtk::Frame = self.glade.get("module_config_hook");
        hook.set_child(gtk::Widget::NONE);
        let Some(page) = self.page() else { return };
        let Some(index) = self.selected_index() else {
            return;
        };
        let kind = page
            .with_contest(|c| c.schedule.stage(index).map(|s| s.kind()))
            .flatten();
        let Some(kind) = kind else { return };
        let screen = {
            let mut screens = self.screens.borrow_mut();
            if index >= screens.len() {
                return;
            }
            let slot = &mut screens[index];
            if slot.is_none() {
                *slot = Some((kind, stages::build(&page, index)));
            }
            slot.as_ref().map(|(_, screen)| screen.clone())
        };
        if let Some(screen) = screen {
            screen.refresh();
            if let Some(configuration) = screen.configuration() {
                Glade::detach_from_parent(&configuration);
                if matches!(kind, StageKind::PoolSupervisor | StageKind::TableSupervisor | StageKind::Barrage | StageKind::Lobby) {
                    hook.set_child(Some(&crate::widgets::bout_timing::wrap(&configuration, &page, index)));
                } else { hook.set_child(Some(&configuration)); }
            }
        }
    }

    /// `on_new_stage_selected` : ajoute une phase après la phase sélectionnée.
    fn add_stage(self: &Rc<Self>, kind: StageKind) {
        let Some(page) = self.page() else { return };
        let after = self.selected_index();
        page.with_contest(|c| {
            c.with_schedule(|s, ctx| {
                s.add_kind(ctx, kind, after);
            })
        });
        page.mark_dirty();
        self.refresh();
    }

    /// `on_stage_removed`.
    fn on_remove(self: &Rc<Self>) {
        let Some(page) = self.page() else { return };
        let Some(index) = self.selected_index() else {
            return;
        };
        page.with_contest(|c| c.with_schedule(|s, ctx| s.remove_user_stage(ctx, index)));
        page.mark_dirty();
        self.refresh();
    }

    fn on_move_up(self: &Rc<Self>) {
        self.move_stage(true);
    }

    fn on_move_down(self: &Rc<Self>) {
        self.move_stage(false);
    }

    /// Monte ou descend la phase sélectionnée (avec sa composition de poules).
    fn move_stage(self: &Rc<Self>, up: bool) {
        let Some(page) = self.page() else { return };
        let Some(index) = self.selected_index() else {
            return;
        };
        let Some(moved) = page.with_contest(|c| c.with_schedule(|s, _| s.move_user_stage(index, up))).flatten() else {
            return;
        };
        // Les écrans des phases déplacées sont reconstruits à leur nouvelle place.
        {
            let mut screens = self.screens.borrow_mut();
            // Seules les phases après la phase courante bougent (avec leur composition).
            let current = page.with_contest(|c| c.schedule.current_index()).unwrap_or(0);
            let from = index.min(moved).saturating_sub(1).max(current + 1);
            for slot in screens.iter_mut().skip(from) {
                if let Some((_, screen)) = slot.take() {
                    if let Some(n) = self.notebook.page_num(&screen.widget()) {
                        self.notebook.remove_page(Some(n));
                    }
                }
            }
        }
        page.mark_dirty();
        self.refresh();
        self.select_index(moved);
        self.refresh_sensitivity();
        self.show_configuration();
    }

    /// `on_previous_stage_toolbutton_clicked`.
    fn on_previous(self: &Rc<Self>) {
        let Some(page) = self.page() else { return };
        let ok = dialogs::confirm(
            page.window().as_ref(),
            &tr("<b><big>Do you really want to cancel the current round?</big></b>"),
            Some(&tr("All the inputs of this round will be cancelled.")),
            Some(&tr("Go back to the previous round?")),
        );
        if !ok {
            return;
        }
        page.with_contest(|c| c.with_schedule(|s, ctx| s.go_back(ctx)));
        page.mark_dirty();
        self.refresh();
    }

    /// `on_next_stage_toolbutton_clicked`.
    fn on_next(self: &Rc<Self>) {
        let Some(page) = self.page() else { return };
        let title = page
            .with_contest(|c| c.schedule.current().map(|s| s.name()))
            .flatten()
            .unwrap_or_default();
        // Archive de la compétition avant de verrouiller la phase
        if let Some(app) = page.app.upgrade() {
            if let Err(e) = app.tournament.borrow_mut().archive_contest(page.id, &title) {
                log::warn!("archive : {e}");
            }
        }
        // Une scission crée une compétition autonome avant le verrouillage,
        // pour pouvoir rester dans cette phase si la duplication échoue.
        let splitting = page.with_contest(|c| c.schedule.current().map(|s| s.kind() == StageKind::Splitting)).flatten().unwrap_or(false);
        let split_contests = if splitting {
            let Some(app) = page.app.upgrade() else { return };
            let result = app.tournament.borrow_mut().split_current_contest(page.id);
            match result {
                Ok(ids) => ids,
                Err(e) => { dialogs::error(page.window().as_ref(), &e.to_string()); return; }
            }
        } else { Vec::new() };
        let exceedance = page
            .with_contest(|c| c.with_schedule(|s, ctx| s.lock_current(ctx)))
            .unwrap_or(0);
        if exceedance > 0 {
            let add = dialogs::run_message(
                page.window().as_ref(),
                gtk::MessageType::Question,
                gtk::ButtonsType::YesNo,
                &tr("<b><big>Because of ties, the quota is exceeded.\nDo you wish to add a barrage round?</big></b>"),
                None,
                Some(&tr("Barrage")),
            ) == gtk::ResponseType::Yes;
            if add {
                page.with_contest(|c| c.with_schedule(|s, ctx| s.add_barrage(ctx)));
            }
        }
        page.with_contest(|c| c.with_schedule(|s, ctx| s.enter_next(ctx)));
        page.mark_dirty();
        self.refresh();
        // Une compétition par division créée ; publiée si la compétition l'est.
        if let Some(app) = page.app.upgrade() {
            let mut derived_pages = Vec::new();
            for id in split_contests {
                if let Some(derived) = app.show_managed_contest(id) {
                    derived.mark_dirty();
                    derived_pages.push(derived);
                }
            }
            crate::publish::publish_split(&app, &page, derived_pages);
        }

    }

    /// `Schedule::OnPrint` : la formule, phase par phase, avec ses réglages.
    fn on_print(self: &Rc<Self>) {
        let Some(page) = self.page() else { return };
        let document = page.with_contest(|c| {
            let team_event = c.settings.team_event;
            let minimum_team_size = c.settings.minimum_team_size;
            let mut lines: Vec<(String, bool)> = Vec::new();
            let count = c.schedule.len();
            for i in 0..count.saturating_sub(1) {
                let Some(stage) = c.schedule.stage(i) else { continue };
                if stage.input_provider().is_some() {
                    continue;
                }
                let shown = if stage.is_input_provider() { c.schedule.stage(i + 1) } else { Some(stage) };
                let Some(shown) = shown else { continue };
                lines.push((format!("{} {}", tr(shown.kind().class_name()), shown.name()), true));

                let qualified = |n: Option<u32>| match n {
                    Some(n) => format!("{} : {}", tr("Qualified"), n),
                    None => format!("{} : 100%", tr("Qualified")),
                };
                if let Some(checkin) = stage.as_any().downcast_ref::<CheckinStage>() {
                    let tally = checkin.tally(&c.players, team_event);
                    lines.push((format!("{} : {}", tr("Expected fencers"), tally.total_fencers), false));
                    if team_event {
                        lines.push((format!("{} : {}", tr("Expected teams"), tally.total_teams), false));
                        lines.push((format!("{} : {}", tr("Team size"), minimum_team_size), false));
                    }
                } else if let Some(allocator) = stage.as_any().downcast_ref::<PoolAllocatorStage>() {
                    let round = allocator.round();
                    let seeding = if round.seeding_is_balanced() { tr("Balanced") } else { tr("Grouped by strength") };
                    lines.push((format!("{} : {}", tr("Seeding"), seeding), false));
                    lines.push((format!("{} : {}", tr("Max score"), round.max_score), false));
                    lines.push((qualified(round.nb_qualified()), false));
                } else if let Some(table) = stage.as_any().downcast_ref::<TableSupervisor>() {
                    let places = match table.fenced_places_mode() {
                        FencedPlaces::AllPlaces => tr("All places fenced"),
                        FencedPlaces::ThirdPlaces => tr("Fence off for third place"),
                        _ => tr("No fence off for third place"),
                    };
                    lines.push((places, false));
                    lines.push((format!("{} : {}", tr("Max score"), table.max_score()), false));
                    lines.push((qualified(table.base().nb_qualified), false));
                }
            }
            let specs = vec![ColumnSpec {
                code: "name".into(),
                title: String::new(),
                look: bp_core::attribute::Look::LongText,
                kind: CellKind::Text,
                editable: false,
            }];
            let rows = lines
                .into_iter()
                .map(|(text, heading)| RowData {
                    player: 0,
                    cells: vec![ListCell::Text(if heading { text } else { format!("    {text}") })],
                    sort: vec![None],
                    children: Vec::new(),
                    dimmed: false,
                })
                .collect();
            ListDocument { title: c.name.clone(), subtitle: tr("Formula"), specs, rows, classification_borders: false }
        });
        let Some(document) = document else { return };
        let window = page.window();
        if let Err(e) = document.print(window.as_ref()) {
            dialogs::error(window.as_ref(), &e.to_string());
        }
    }
}
